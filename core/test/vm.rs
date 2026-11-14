// Tests of vm.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::{
    EMULATED_KERNEL_ARGS, console_excerpt, ensure_emulated_cmdline, extend_linux_cmdline,
    largest_partition_offset,
};
use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
    sync::OnceLock,
};

/// The container that runs the disk image tools for these tests (see `command` in vm.rs).
static TOOLS: OnceLock<String> = OnceLock::new();

/// `program` in the tools container. vm.rs runs its disk image tools through this in tests.
pub fn command(program: &str) -> Command {
    let container = TOOLS
        .get()
        .expect("no tools container: call with_tools first");
    let mut command = Command::new("docker");
    command.args(["exec", "-i", container, program]);
    command
}

/// Starts a container with qemu-img and e2fsprogs that sees `dir` at the same path, as the user
/// who owns `dir`, so a file either side writes the other can change. Removed when dropped.
fn with_tools(dir: &Path) -> RemoveContainer {
    crate::tests::require_docker();
    let image = "bettertest-test-disk-tools";
    let mut build = Command::new("docker")
        .args(["build", "--quiet", "--tag", image, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let dockerfile = b"FROM debian:trixie\n\
        RUN apt-get update && apt-get install -y --no-install-recommends e2fsprogs qemu-utils\n";
    build.stdin.take().unwrap().write_all(dockerfile).unwrap();
    assert!(build.wait().unwrap().success(), "could not build {image}");
    let owner = fs::metadata(dir).unwrap();
    let user = format!("{}:{}", owner.uid(), owner.gid());
    let mount = format!("{0}:{0}:z", dir.display());
    let run = Command::new("docker")
        .args([
            "run", "--detach", "--rm", "--init", "--user", &user, "--volume", &mount,
        ])
        .args([image, "sleep", "infinity"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let id = String::from_utf8(run.stdout).unwrap().trim().to_string();
    TOOLS.set(id.clone()).unwrap();
    RemoveContainer(id)
}

struct RemoveContainer(String);

impl Drop for RemoveContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.0])
            .output();
    }
}

#[test]
fn linux_command_gains_the_emulated_timeout_once() {
    let cfg = "\
menuentry 'Debian GNU/Linux' {\n\
\tlinux\t/boot/vmlinuz root=PARTUUID=abc ro  \n\
\tinitrd\t/boot/initrd.img\n\
}\n\
menuentry 'recovery' {\n\
\tlinuxefi /boot/vmlinuz root=PARTUUID=abc ro single\n\
}\n";
    let once = extend_linux_cmdline(cfg, EMULATED_KERNEL_ARGS).unwrap();
    assert!(once.contains(&format!("ro {EMULATED_KERNEL_ARGS}")));
    assert!(once.contains(&format!("single {EMULATED_KERNEL_ARGS}")));
    assert!(once.contains("initrd"));
    let twice = extend_linux_cmdline(&once, EMULATED_KERNEL_ARGS).unwrap();
    assert_eq!(once, twice);
    let err = extend_linux_cmdline("set default=0\n", EMULATED_KERNEL_ARGS).unwrap_err();
    assert!(err.contains("no linux command"), "{err}");
}

#[test]
fn console_excerpt_keeps_the_failure_under_apparmor_spam() {
    let mut text = String::from("Timed out waiting for device dev-disk-by-partuuid-efi.device\n");
    text.push_str("Dependency failed for boot-efi.mount - /boot/efi.\n");
    text.push_str("local-fs.target: Triggering OnFailure= dependencies.\n");
    text.push_str("You are in emergency mode.\n");
    for i in 0..50 {
        text.push_str(&format!(
            "[  {i}.0] audit: apparmor=\"STATUS\" operation=\"profile_load\" name=\"p{i}\"\n"
        ));
    }
    let excerpt = console_excerpt(&text);
    assert!(excerpt.contains("boot-efi.mount"));
    assert!(excerpt.contains("emergency mode"));
    assert!(!excerpt.contains("profile_load"));
}

#[test]
fn largest_partition_is_the_root_not_the_efi_one() {
    let disk = gpt(&[(2048, 262143), (262144, 6289407)]);
    let offset = largest_partition_offset(&disk).unwrap();
    assert_eq!(offset, 262144 * 512);
    let err = largest_partition_offset(&[0; 2048]).unwrap_err();
    assert!(err.contains("no GPT"), "{err}");
}

/// debugfs rewrite of a real grub.cfg inside a qcow2, the same path a worker takes. The disk tools
/// run in a container, so all this needs from the machine is docker.
#[test]
fn emulated_cmdline_is_written_into_the_image() {
    let dir = Path::new("/var/tmp/bettertest/vm-cmdline-test");
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).unwrap();
    let _tools = with_tools(dir);
    let disk = dir.join("disk.raw");
    let root = dir.join("root.img");
    let grub = dir.join("grub.cfg");
    let qcow = dir.join("image.qcow2");
    fs::write(&disk, vec![0u8; 48 * 1024 * 1024]).unwrap();
    // The small partition stands in for the EFI system partition. The root is the 32 MiB one at
    // 1 MiB, which is where the ext4 image is written.
    let table = gpt(&[(34, 2047), (2048, 2048 + 65536 - 1)]);
    let mut disk_bytes = fs::read(&disk).unwrap();
    disk_bytes[..table.len()].copy_from_slice(&table);
    fs::write(&disk, disk_bytes).unwrap();
    assert!(
        command("mkfs.ext4")
            .args(["-q", "-F", "-L", "rootfs"])
            .arg(&root)
            .arg("32M")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        command("dd")
            .args([
                &format!("if={}", root.display()),
                &format!("of={}", disk.display()),
                "bs=1M",
                "seek=1",
                "conv=notrunc",
            ])
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        &grub,
        "menuentry 'Debian' {\n\tlinux\t/boot/vmlinuz root=PARTUUID=abc ro\n\tinitrd\t/boot/initrd.img\n}\n",
    )
    .unwrap();
    let spec = format!("{}?offset={}", disk.display(), 2048 * 512);
    for request in ["mkdir /boot", "mkdir /boot/grub"] {
        let _ = command("debugfs")
            .args(["-w", "-R", request, &spec])
            .status();
    }
    let planted = command("debugfs")
        .args([
            "-w",
            "-R",
            &format!("write {} /boot/grub/grub.cfg", grub.display()),
            &spec,
        ])
        .output()
        .unwrap();
    assert!(
        planted.status.success(),
        "{}",
        String::from_utf8_lossy(&planted.stdout)
    );
    // The failure seen on a real image: link the file onto itself the way debugfs ln does, so
    // grub.cfg names a deleted inode. Rewriting has to give it a new one, not only the same bytes.
    let linked = command("debugfs")
        .args([
            "-w",
            "-R",
            "ln /boot/grub/grub.cfg /boot/grub/grub.cfg.linked",
            &spec,
        ])
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let removed = command("debugfs")
        .args(["-w", "-R", "rm /boot/grub/grub.cfg.linked", &spec])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let broken = command("e2fsck").args(["-fn", &spec]).output().unwrap();
    assert!(!broken.status.success(), "the fixture should start corrupt");
    assert!(
        command("qemu-img")
            .args([
                "convert",
                "-f",
                "raw",
                "-O",
                "qcow2",
                &disk.to_string_lossy(),
                &qcow.to_string_lossy(),
            ])
            .status()
            .unwrap()
            .success()
    );

    ensure_emulated_cmdline(&qcow).unwrap();
    let patched = dir.join("patched.raw");
    assert!(
        command("qemu-img")
            .args([
                "convert",
                "-f",
                "qcow2",
                "-O",
                "raw",
                &qcow.to_string_lossy(),
                &patched.to_string_lossy(),
            ])
            .status()
            .unwrap()
            .success()
    );
    let spec = format!("{}?offset={}", patched.display(), 2048 * 512);
    let read = command("debugfs")
        .args(["-R", "cat /boot/grub/grub.cfg", &spec])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&read.stdout);
    assert!(
        text.contains(EMULATED_KERNEL_ARGS),
        "stdout {}\nstderr {}",
        text,
        String::from_utf8_lossy(&read.stderr)
    );
    // A new file must not be handed grub.cfg's inode. That is what made reboot stop at grub>.
    let extra = dir.join("extra.txt");
    fs::write(&extra, "not grub\n").unwrap();
    let wrote = command("debugfs")
        .args([
            "-w",
            "-R",
            &format!("write {} /extra.txt", extra.display()),
            &spec,
        ])
        .output()
        .unwrap();
    assert!(
        wrote.status.success(),
        "{}",
        String::from_utf8_lossy(&wrote.stderr)
    );
    let check = command("e2fsck").args(["-fn", &spec]).output().unwrap();
    assert!(
        check.status.success(),
        "stdout {}\nstderr {}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    let read = command("debugfs")
        .args(["-R", "cat /boot/grub/grub.cfg", &spec])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&read.stdout);
    assert!(text.contains(EMULATED_KERNEL_ARGS), "{text}");
    // The marker makes a second boot skip the rewrite.
    let before = fs::metadata(&qcow).unwrap().modified().unwrap();
    ensure_emulated_cmdline(&qcow).unwrap();
    assert_eq!(fs::metadata(&qcow).unwrap().modified().unwrap(), before);
    let _ = fs::remove_dir_all(dir);
}

fn gpt(parts: &[(u64, u64)]) -> Vec<u8> {
    let mut disk = vec![0u8; 512 * (2 + 128)];
    disk[512..520].copy_from_slice(b"EFI PART");
    disk[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
    disk[512 + 80..512 + 84].copy_from_slice(&128u32.to_le_bytes());
    disk[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
    for (i, (first, last)) in parts.iter().enumerate() {
        let at = 512 * 2 + i * 128;
        disk[at] = 1;
        disk[at + 32..at + 40].copy_from_slice(&first.to_le_bytes());
        disk[at + 40..at + 48].copy_from_slice(&last.to_le_bytes());
    }
    disk
}
