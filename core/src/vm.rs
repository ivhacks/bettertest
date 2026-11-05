// Virtual machines, without root. qemu's user-mode networking needs no bridge, tap device or setuid
// helper: the vm reaches the worker at 10.0.2.2, and the worker reaches the vm's ssh through a port
// forwarded on 127.0.0.1. cloud-init inside the vm fetches its config from the worker and installs a
// unit that calls back every time the vm finishes booting, so nothing waits on a timer or retries
// ssh until it works.

use crate::{lock, workdir, worker};
use bettertest_shared::Arch;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tokio::{
    fs::File,
    io::AsyncWriteExt,
    process::{Child, Command},
    sync::oneshot,
    time::sleep,
};
use uuid::Uuid;

const BOOT_TIMEOUT: Duration = Duration::from_secs(300);

pub struct Vms {
    // Where this worker listens, so a vm can call back at 10.0.2.2.
    port: u16,
    // cloud-init may ask for its config on any boot, so it is served for the vm's whole life.
    cloud_init: Mutex<HashMap<String, CloudInit>>,
    // Who is waiting for a vm to finish booting, the first time or after a reboot.
    waiting: Mutex<HashMap<String, oneshot::Sender<()>>>,
    running: Mutex<HashMap<String, Vm>>,
}

struct CloudInit {
    user_data: String,
    meta_data: String,
}

struct Vm {
    qemu: Child,
    dir: PathBuf,
    ssh_port: u16,
}

impl Vms {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            cloud_init: Mutex::new(HashMap::new()),
            waiting: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Boots a vm from the qcow2 image at `image_url` and waits until it is ready for ssh. `arch`
    /// is the image's architecture, this worker's own when None.
    pub async fn start(&self, image_url: &str, arch: Option<Arch>) -> Result<String, String> {
        let qemu = qemu(arch.unwrap_or(NATIVE))?;
        let base = cached_image(image_url).await?;
        let id = Uuid::new_v4().simple().to_string();
        // 0777 so the next unix user's worker can clean it up if this worker dies.
        let dir = workdir::dir("vms").map_err(|e| e.to_string())?.join(&id);
        workdir::shared_dir(&dir).map_err(|e| e.to_string())?;

        match self.boot(&id, &dir, &base, qemu).await {
            Ok(vm) => {
                lock(&self.running).insert(id.clone(), vm);
                Ok(id)
            }
            Err(e) => {
                lock(&self.cloud_init).remove(&id);
                lock(&self.waiting).remove(&id);
                let _ = fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    async fn boot(
        &self,
        id: &str,
        dir: &Path,
        base: &Path,
        mut qemu: Command,
    ) -> Result<Vm, String> {
        let key = dir.join("key");
        worker::output(
            Command::new("ssh-keygen")
                .arg("-q")
                .args(["-t", "ed25519", "-N", ""])
                .arg("-f")
                .arg(&key),
        )
        .await?;
        let public_key = fs::read_to_string(dir.join("key.pub")).map_err(|e| e.to_string())?;

        let disk = dir.join("disk.qcow2");
        // A copy-on-write overlay: the vm writes here, the shared base image never changes.
        worker::output(
            Command::new("qemu-img")
                .args(["create", "-q", "-f", "qcow2", "-F", "qcow2", "-b"])
                .arg(base)
                .arg(&disk),
        )
        .await?;

        let ssh_port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        let host = format!("http://10.0.2.2:{}/vm/{id}", self.port);
        lock(&self.cloud_init).insert(
            id.to_string(),
            CloudInit {
                user_data: user_data(public_key.trim(), &host),
                meta_data: format!("instance-id: {id}\nlocal-hostname: bettertest-vm\n"),
            },
        );
        let booted = self.wait_for_boot(id);

        let console = dir.join("console.log");
        let qemu_log = dir.join("qemu.log");
        let stderr = fs::File::create(&qemu_log).map_err(|e| e.to_string())?;
        let program = qemu.as_std().get_program().to_string_lossy().into_owned();
        let mut qemu = qemu
            // kvm when this user may use /dev/kvm and the vm is this machine's architecture,
            // otherwise plain emulation. Slow, but it boots.
            .args([
                "-accel", "kvm", "-accel", "tcg", "-cpu", "max", "-m", "2048", "-smp", "2",
            ])
            .args(["-display", "none", "-serial"])
            .arg(format!("file:{}", console.display()))
            .arg("-drive")
            .arg(format!("file={},if=virtio", disk.display()))
            .args([
                "-netdev",
                &format!("user,id=net,hostfwd=tcp:127.0.0.1:{ssh_port}-:22"),
            ])
            .args(["-device", "virtio-net-pci,netdev=net"])
            // Tells cloud-init to fetch user-data and meta-data from the worker.
            .args([
                "-smbios",
                &format!("type=1,serial=ds=nocloud-net;s={host}/"),
            ])
            .arg("-pidfile")
            .arg(dir.join("qemu.pid"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("could not start {program}: {e}"))?;

        tokio::select! {
            _ = booted => Ok(Vm { qemu, dir: dir.to_path_buf(), ssh_port }),
            _ = qemu.wait() => {
                let stderr = fs::read_to_string(&qemu_log).unwrap_or_default();
                Err(format!("qemu exited while booting: {stderr}{}", console_tail(&console)))
            }
            _ = sleep(BOOT_TIMEOUT) => {
                Err(format!("vm did not finish booting within {BOOT_TIMEOUT:?}{}", console_tail(&console)))
            }
        }
    }

    /// Reboots the vm cleanly and waits until it has finished booting again.
    pub async fn reboot(&self, id: &str) -> Result<(), String> {
        let Some((ssh_port, key)) = self.ssh(id) else {
            return Err(format!("no vm {id}"));
        };
        let booted = self.wait_for_boot(id);
        // The connection drops as the vm goes down, so ssh's exit status says nothing either way.
        let reboot = Command::new("ssh")
            .args(SSH_OPTIONS)
            .arg("-i")
            .arg(&key)
            .args([
                "-p",
                &ssh_port.to_string(),
                "bt@127.0.0.1",
                "sudo systemctl reboot",
            ])
            .stdin(Stdio::null())
            .output();
        let _ = tokio::time::timeout(Duration::from_secs(60), reboot).await;
        tokio::select! {
            _ = booted => Ok(()),
            _ = sleep(BOOT_TIMEOUT) => {
                lock(&self.waiting).remove(id);
                let console = console_tail(&key.with_file_name("console.log"));
                Err(format!("vm {id} did not come back within {BOOT_TIMEOUT:?}{console}"))
            }
        }
    }

    /// Resolves when the vm next says it has finished booting.
    fn wait_for_boot(&self, id: &str) -> oneshot::Receiver<()> {
        let (ready, booted) = oneshot::channel();
        lock(&self.waiting).insert(id.to_string(), ready);
        booted
    }

    pub async fn delete(&self, id: &str) -> Result<(), String> {
        let Some(mut vm) = lock(&self.running).remove(id) else {
            return Err(format!("no vm {id}"));
        };
        lock(&self.cloud_init).remove(id);
        lock(&self.waiting).remove(id);
        let _ = vm.qemu.kill().await;
        fs::remove_dir_all(&vm.dir).map_err(|e| e.to_string())
    }

    /// The vm's ssh port on 127.0.0.1, and the private key that logs in as `bt`.
    pub fn ssh(&self, id: &str) -> Option<(u16, PathBuf)> {
        lock(&self.running)
            .get(id)
            .map(|vm| (vm.ssh_port, vm.dir.join("key")))
    }

    pub fn cloud_init(&self, id: &str, file: &str) -> Option<String> {
        let cloud_init = lock(&self.cloud_init);
        let vm = cloud_init.get(id)?;
        match file {
            "user-data" => Some(vm.user_data.clone()),
            "meta-data" => Some(vm.meta_data.clone()),
            _ => None,
        }
    }

    /// The vm finished booting. Returns false when nobody is waiting for it.
    pub fn ready(&self, id: &str) -> bool {
        match lock(&self.waiting).remove(id) {
            Some(ready) => ready.send(()).is_ok(),
            None => false,
        }
    }
}

/// This worker's own architecture, which kvm runs at full speed.
#[cfg(target_arch = "aarch64")]
pub const NATIVE: Arch = Arch::Aarch64;
#[cfg(not(target_arch = "aarch64"))]
pub const NATIVE: Arch = Arch::X86_64;

// UEFI firmware for arm vms, where Fedora (edk2-aarch64) and Debian (qemu-efi-aarch64) put it.
const AARCH64_FIRMWARE: [&str; 2] = [
    "/usr/share/edk2/aarch64/QEMU_EFI.fd",
    "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
];

/// qemu for a vm of `arch`, with the machine it needs.
fn qemu(arch: Arch) -> Result<Command, String> {
    match arch {
        Arch::X86_64 => Ok(Command::new("qemu-system-x86_64")),
        Arch::Aarch64 => {
            let firmware = AARCH64_FIRMWARE
                .iter()
                .find(|path| Path::new(path).exists())
                .ok_or_else(|| {
                    format!(
                        "arm vms need UEFI firmware at {} (edk2-aarch64 or qemu-efi-aarch64)",
                        AARCH64_FIRMWARE.join(" or ")
                    )
                })?;
            let mut qemu = Command::new("qemu-system-aarch64");
            // virt is qemu's machine made for vms; arm boots a disk image through UEFI. With
            // highmem=off everything sits below 4 GiB, which an M1's kvm can map.
            qemu.args(["-M", "virt,gic-version=max,highmem=off", "-bios", firmware]);
            Ok(qemu)
        }
    }
}

fn user_data(public_key: &str, host: &str) -> String {
    // bettertest-ready runs at the end of every boot, after sshd is up, and tells the worker. On the
    // first boot, runcmd starts sshd before enabling it. Debian's ssh.service also answers to
    // sshd.service, the name everywhere else; python3 is there because cloud-init itself needs it.
    format!(
        "#cloud-config
users:
  - name: bt
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - {public_key}
write_files:
  - path: /etc/systemd/system/bettertest-ready.service
    content: |
      [Unit]
      Description=Tell the bettertest worker this vm has booted
      Wants=network-online.target
      After=network-online.target ssh.service sshd.service

      [Service]
      Type=oneshot
      ExecStart=/usr/bin/python3 -c \"import urllib.request as r; r.urlopen(r.Request('{host}/ready', method='POST'))\"

      [Install]
      WantedBy=multi-user.target
runcmd:
  - [systemctl, start, sshd]
  - [systemctl, enable, --now, bettertest-ready.service]
"
    )
}

/// Ignore ~/.ssh/config, agents and known_hosts: the vm is brand new and only our key works. The
/// python library passes the same options.
const SSH_OPTIONS: [&str; 12] = [
    "-F",
    "/dev/null",
    "-o",
    "IdentitiesOnly=yes",
    "-o",
    "IdentityAgent=none",
    "-o",
    "StrictHostKeyChecking=no",
    "-o",
    "UserKnownHostsFile=/dev/null",
    "-o",
    "LogLevel=ERROR",
];

fn console_tail(console: &Path) -> String {
    let text = fs::read_to_string(console).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(30)..].join("\n");
    format!("\nlast lines of the vm console:\n{tail}")
}

/// Downloads the image at `url` the first time it is asked for. The copy is keyed by that URL, so
/// a later change at the same URL is not fetched again.
async fn cached_image(url: &str) -> Result<PathBuf, String> {
    let images = workdir::dir("vm-images").map_err(|e| e.to_string())?;
    let path = images.join(format!("{}.qcow2", worker::cache_key(url)));
    if path.exists() {
        return Ok(path);
    }

    let mut response = worker::download(url).await?;
    // Download beside the final name and rename when complete, so a half-written image is never
    // mistaken for a cached one.
    let partial = images.join(format!("{}.part", Uuid::new_v4().simple()));
    let file = workdir::new_shared_file(&partial).map_err(|e| e.to_string())?;
    let mut file = File::from_std(file);
    let downloaded = async {
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        file.flush().await.map_err(|e| e.to_string())
    };
    if let Err(e) = downloaded.await {
        let _ = fs::remove_file(&partial);
        return Err(format!("could not download {url}: {e}"));
    }
    fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Stops vms and deletes vm directories left behind by a worker process that died.
pub fn remove_leftovers() -> std::io::Result<()> {
    for entry in fs::read_dir(workdir::dir("vms")?)? {
        let dir = entry?.path();
        // Only kill the pid if it is still that qemu. After a reboot the number may belong to
        // something else entirely.
        if let Ok(pid) = fs::read_to_string(dir.join("qemu.pid")) {
            let pid = pid.trim();
            let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if String::from_utf8_lossy(&cmdline).contains(&*dir.to_string_lossy()) {
                std::process::Command::new("kill").arg(pid).status()?;
            }
        }
        println!("removing leftover vm {}", dir.display());
        fs::remove_dir_all(&dir)?;
    }
    Ok(())
}
