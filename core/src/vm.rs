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
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
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

// kvm. An emulated guest is a different wait: its clock runs at wall speed while the cpu does not.
const NATIVE_BOOT_TIMEOUT: Duration = Duration::from_secs(300);
const EMULATED_BOOT_TIMEOUT: Duration = Duration::from_secs(1800);

// Debian cloud requires /boot/efi from local-fs.target, and that target's failure is emergency
// mode. sulogin then stops, because the image's root account is locked, so cloud-init never runs.
// The wait for the EFI partition is systemd's device timeout, 90s. On a small host, TCG still has
// not created the by-partuuid link by then (a fast machine does it in well under 90s). The start
// timeout covers units such as udev that are on the same clock. Both are kernel command line
// options, so they apply before that mount is attempted.
const EMULATED_KERNEL_ARGS: &str =
    "systemd.default_device_timeout_sec=900 systemd.default_timeout_start_sec=900";

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
    boot_timeout: Duration,
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
        let arch = arch.unwrap_or(NATIVE);
        let qemu = qemu(arch)?;
        let base = cached_image(image_url).await?;
        // The guest reads this off its own disk, before systemd starts the mount that would fail.
        if arch != NATIVE {
            let image = base.clone();
            match tokio::task::spawn_blocking(move || ensure_emulated_cmdline(&image)).await {
                Ok(result) => result?,
                Err(_) => return Err("could not prepare the emulated vm image".to_string()),
            }
        }
        let id = Uuid::new_v4().simple().to_string();
        // 0777 so the next unix user's worker can clean it up if this worker dies.
        let dir = workdir::dir("vms").map_err(|e| e.to_string())?.join(&id);
        workdir::shared_dir(&dir).map_err(|e| e.to_string())?;

        let timeout = if arch == NATIVE {
            NATIVE_BOOT_TIMEOUT
        } else {
            EMULATED_BOOT_TIMEOUT
        };
        match self.boot(&id, &dir, &base, qemu, timeout).await {
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
        timeout: Duration,
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
            // TCG has no hardware rng. cloud-init and sshd wait on /dev/random without this.
            .args(["-device", "virtio-rng-pci"])
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
            _ = booted => Ok(Vm { qemu, dir: dir.to_path_buf(), ssh_port, boot_timeout: timeout }),
            _ = qemu.wait() => {
                let stderr = fs::read_to_string(&qemu_log).unwrap_or_default();
                Err(format!("qemu exited while booting: {stderr}{}", console_tail(&console)))
            }
            _ = sleep(timeout) => {
                Err(format!("vm did not finish booting within {timeout:?}{}", console_tail(&console)))
            }
        }
    }

    /// Reboots the vm cleanly and waits until it has finished booting again.
    pub async fn reboot(&self, id: &str) -> Result<(), String> {
        let (ssh_port, key, timeout) = {
            let running = lock(&self.running);
            let Some(vm) = running.get(id) else {
                return Err(format!("no vm {id}"));
            };
            (vm.ssh_port, vm.dir.join("key"), vm.boot_timeout)
        };
        let booted = self.wait_for_boot(id);
        // --no-block returns once systemd has queued the reboot. Without it, systemctl waits out
        // the shutdown and the ssh session dies with the guest. Dropping that wait kills ssh,
        // and a signal on that session can cancel the reboot before it happens.
        let reboot = Command::new("ssh")
            .args(SSH_OPTIONS)
            .arg("-i")
            .arg(&key)
            .args([
                "-p",
                &ssh_port.to_string(),
                "bt@127.0.0.1",
                "sudo systemctl reboot --no-block",
            ])
            .stdin(Stdio::null())
            .output();
        let output = match tokio::time::timeout(Duration::from_secs(60), reboot).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("could not run ssh to reboot vm {id}: {e}")),
            Err(_) => {
                return Err(format!(
                    "vm {id} did not accept reboot within 60s (ssh did not return)"
                ));
            }
        };
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            };
            return Err(format!(
                "vm {id} did not reboot: ssh exited {}{detail}",
                output.status
            ));
        }
        tokio::select! {
            _ = booted => Ok(()),
            _ = sleep(timeout) => {
                lock(&self.waiting).remove(id);
                let console = console_tail(&key.with_file_name("console.log"));
                Err(format!("vm {id} did not come back within {timeout:?}{console}"))
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
    format!(
        "\nlast lines of the vm console:\n{}",
        console_excerpt(&text)
    )
}

/// AppArmor logs one line per profile. On a slow boot those lines are the whole tail, and the
/// line that says why the boot stopped is gone.
fn console_excerpt(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !(line.contains("apparmor=\"STATUS\"") && line.contains("profile_load")))
        .collect();
    let start = lines.len().saturating_sub(40);
    lines[start..].join("\n")
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

/// A marker from a worker that linked grub.cfg over itself. debugfs `ln` does not keep the link
/// count, so removing the temporary name deletes the inode while the directory entry stays. The
/// first boot still reads the old blocks. The guest then reuses that inode (its journal directory)
/// and the next boot stops at `grub>` with `can't find command ''`.
const CMDLINE_STAMP: &str =
    "v2\nsystemd.default_device_timeout_sec=900 systemd.default_timeout_start_sec=900\n";

/// Puts `EMULATED_KERNEL_ARGS` on the image's kernel command line, once. Overlays created after
/// this see it. Two boots of one image must not both rewrite it.
fn ensure_emulated_cmdline(image: &Path) -> Result<(), String> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let marker = sibling(image, "emulated-cmdline")?;
    if marker_is_current(image, &marker) {
        return Ok(());
    }
    let raw = RemoveOnDrop(sibling(image, "raw")?);
    let rewritten = RemoveOnDrop(sibling(image, "new.qcow2")?);
    let grub_file = RemoveOnDrop(sibling(image, "grub.cfg")?);
    let convert = |from: &str, to: &str, input: &Path, output: &Path| {
        let (input, output) = (input.to_string_lossy(), output.to_string_lossy());
        tool(
            "qemu-img",
            &["convert", "-f", from, "-O", to, &input, &output],
        )
    };
    convert("qcow2", "raw", image, &raw.0)?;
    let offset = largest_partition_offset(&read_prefix(&raw.0, 1024 * 1024)?)?;
    let spec = format!("{}?offset={offset}", raw.0.display());
    // Debian's grub.cfg and Fedora's grub2 one. The edit is on a raw copy; the cached qcow is
    // replaced only after e2fsck accepts it, so a crash mid-edit leaves the previous image.
    let Some((cfg_path, grub)) = find_grub(&spec)? else {
        write_marker(&marker)?;
        return Ok(());
    };
    // Always write a new inode. An image whose text is already right can still have grub.cfg
    // pointing at a deleted inode, and skipping the write would stamp that image as done.
    let updated = extend_linux_cmdline(&grub, EMULATED_KERNEL_ARGS)?;
    fs::write(&grub_file.0, &updated).map_err(|e| e.to_string())?;
    replace_grub(&spec, &cfg_path, &grub_file.0)?;
    // It has to pass a read-only check: a directory entry aimed at a deleted inode still reads
    // back the old bytes, so reading grub.cfg is not enough.
    tool("e2fsck", &["-fn", &spec])
        .map_err(|e| format!("rewritten vm image is not a consistent filesystem: {e}"))?;
    convert("raw", "qcow2", &raw.0, &rewritten.0)?;
    fs::set_permissions(&rewritten.0, fs::Permissions::from_mode(0o666))
        .map_err(|e| e.to_string())?;
    fs::rename(&rewritten.0, image).map_err(|e| e.to_string())?;
    write_marker(&marker)
}

fn marker_is_current(image: &Path, marker: &Path) -> bool {
    let modified = |path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let (Some(image_time), Some(marker_time)) = (modified(image), modified(marker)) else {
        return false;
    };
    marker_time >= image_time && fs::read_to_string(marker).ok().as_deref() == Some(CMDLINE_STAMP)
}

fn write_marker(marker: &Path) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o666)
        .open(marker)
        .and_then(|mut file| file.write_all(CMDLINE_STAMP.as_bytes()))
        .map_err(|e| e.to_string())
}

fn sibling(image: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = image
        .file_name()
        .ok_or("vm image has no file name")?
        .to_string_lossy();
    Ok(image.with_file_name(format!(".{name}.{suffix}")))
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn read_prefix(path: &Path, len: usize) -> Result<Vec<u8>, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0; len];
    use std::io::Read;
    let n = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    Ok(buf)
}

/// Byte offset of the largest GPT partition. A cloud image's root filesystem is that partition;
/// the EFI one is much smaller.
fn largest_partition_offset(disk: &[u8]) -> Result<u64, String> {
    if disk.len() < 1024 || &disk[512..520] != b"EFI PART" {
        return Err("vm image has no GPT, so its boot timeout cannot be set".to_string());
    }
    let entry_lba = u64::from_le_bytes(le(disk, 512 + 72)?);
    let nent = u32::from_le_bytes(le(disk, 512 + 80)?);
    let entsz = u32::from_le_bytes(le(disk, 512 + 84)?);
    if nent == 0 || nent > 128 || !(128..=512).contains(&entsz) {
        return Err("vm image has a GPT this worker will not parse".to_string());
    }
    let start = entry_lba
        .checked_mul(512)
        .ok_or("vm image partition table is too large")?;
    let bytes = (nent as u64)
        .checked_mul(entsz as u64)
        .ok_or("vm image partition table is too large")?;
    let end = start
        .checked_add(bytes)
        .ok_or("vm image partition table is too large")?;
    if end > disk.len() as u64 {
        return Err("vm image partition table is past the start of the disk".to_string());
    }
    let mut best: Option<(u64, u64)> = None;
    for i in 0..nent {
        let at = (start + i as u64 * entsz as u64) as usize;
        let entry = &disk[at..at + entsz as usize];
        if entry[..16].iter().all(|b| *b == 0) {
            continue;
        }
        let first = u64::from_le_bytes(le(entry, 32)?);
        let last = u64::from_le_bytes(le(entry, 40)?);
        if last < first {
            continue;
        }
        let size = last - first + 1;
        if best.is_none_or(|(_, got)| size > got) {
            best = Some((first, size));
        }
    }
    let Some((first, _)) = best else {
        return Err("vm image GPT has no partitions".to_string());
    };
    first
        .checked_mul(512)
        .ok_or_else(|| "vm image partition offset overflows".to_string())
}

/// The `N` bytes at `at`, for from_le_bytes.
fn le<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], String> {
    let slice = bytes.get(at..at + N).and_then(|s| s.try_into().ok());
    slice.ok_or_else(|| "truncated partition table".to_string())
}

/// Runs `program` to its end. Ok is its stdout; Err says what went wrong.
fn tool(program: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    match output.status.success() {
        true => Ok(stdout),
        false => Err(format!(
            "{program} failed: {}{stdout}",
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

fn debugfs(spec: &str, request: &str, write: bool) -> Result<String, String> {
    let mut command = std::process::Command::new("debugfs");
    if write {
        command.arg("-w");
    }
    let output = command
        .args(["-R", request, spec])
        .output()
        .map_err(|e| format!("could not run debugfs (e2fsprogs): {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(format!("debugfs failed: {stderr}{stdout}"));
    }
    for needle in [
        "Filesystem not open",
        "Bad magic",
        "Ext2 file already exists",
        "invalid option",
    ] {
        if stdout.contains(needle) || stderr.contains(needle) {
            return Err(format!("debugfs failed: {stderr}{stdout}"));
        }
    }
    Ok(stdout)
}

fn debugfs_cat(spec: &str, path: &str) -> Result<String, String> {
    let stdout = debugfs(spec, &format!("cat {path}"), false)?;
    if stdout.contains("File not found") {
        return Err(format!("debugfs: File not found: {path}"));
    }
    Ok(without_debugfs_banner(&stdout).to_string())
}

fn without_debugfs_banner(stdout: &str) -> &str {
    match stdout.strip_prefix("debugfs ") {
        Some(rest) => rest.split_once('\n').map(|(_, body)| body).unwrap_or(""),
        None => stdout,
    }
}

/// `(path, contents)` of the image's grub.cfg. None when it has none to edit.
fn find_grub(spec: &str) -> Result<Option<(String, String)>, String> {
    for path in ["/boot/grub/grub.cfg", "/boot/grub2/grub.cfg"] {
        match debugfs_cat(spec, path) {
            Ok(text) => return Ok(Some((path.to_string(), text))),
            Err(e) if e.contains("File not found") => {
                let staged = format!("{path}.bettertest");
                if let Ok(text) = debugfs_cat(spec, &staged) {
                    return Ok(Some((path.to_string(), text)));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Replaces grub.cfg with the host file. The old name is removed first, then the file is written
/// as a new inode. debugfs `ln` does not keep a link count, and writing a second name while the
/// old entry still points at a deleted inode makes debugfs reuse that inode and the entry stays
/// deleted. Reading the file cannot tell: a deleted inode still returns its old bytes.
fn replace_grub(spec: &str, cfg_path: &str, host_file: &Path) -> Result<(), String> {
    let _ = debugfs(spec, &format!("rm {cfg_path}"), true);
    let wrote = debugfs(
        spec,
        &format!("write {} {cfg_path}", host_file.display()),
        true,
    )?;
    if wrote.contains("File not found") || wrote.contains("already exists") {
        return Err(format!("debugfs could not write {cfg_path}: {wrote}"));
    }
    let final_text = debugfs_cat(spec, cfg_path)?;
    if !final_text.contains(EMULATED_KERNEL_ARGS) {
        return Err("grub.cfg is missing the emulated boot timeout".to_string());
    }
    Ok(())
}

/// Adds `extra` to every grub `linux` / `linuxefi` command. Existing copies of these timeouts are
/// replaced, so a later change of the values does not stack.
fn extend_linux_cmdline(cfg: &str, extra: &str) -> Result<String, String> {
    let mut changed = false;
    let mut out = String::new();
    for line in cfg.split_inclusive('\n') {
        let without_nl = line.trim_end_matches('\n');
        let trimmed = without_nl.trim_start();
        if is_kernel_line(trimmed) {
            let indent_len = without_nl.len() - trimmed.len();
            out.push_str(&without_nl[..indent_len]);
            out.push_str(&strip_timeout_args(trimmed));
            out.push(' ');
            out.push_str(extra);
            changed = true;
        } else {
            out.push_str(without_nl);
        }
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    changed.then_some(out).ok_or_else(|| {
        "grub.cfg has no linux command, so the emulated boot timeout cannot be set".to_string()
    })
}

fn is_kernel_line(trimmed: &str) -> bool {
    (trimmed.split_once([' ', '\t']))
        .is_some_and(|(command, _)| matches!(command, "linux" | "linuxefi"))
}

fn strip_timeout_args(line: &str) -> String {
    line.split_whitespace()
        .filter(|word| {
            !word.starts_with("systemd.default_device_timeout_sec=")
                && !word.starts_with("systemd.default_timeout_start_sec=")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "../test/vm.rs"]
mod tests;
