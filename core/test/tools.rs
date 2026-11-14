// Tests of tools.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::path;
use std::{os::unix::fs::PermissionsExt, process::Command};

/// Each program runs from where it was written out, with nothing from the machine: a dynamically
/// linked one would need the machine's libc, and its loader on the machine's path.
#[test]
fn every_tool_runs_static_from_the_directory_it_is_unpacked_into() {
    for (tool, flag, says) in [
        ("qemu-system-x86_64", "--version", "QEMU emulator version"),
        ("qemu-system-aarch64", "--version", "QEMU emulator version"),
        ("qemu-img", "--version", "qemu-img version"),
        ("debugfs", "-V", "debugfs"),
        ("e2fsck", "-V", "e2fsck"),
        ("ssh", "-V", "OpenSSH"),
        ("ssh-keygen", "--help", "usage: ssh-keygen"),
        ("scp", "--help", "usage: scp"),
    ] {
        let program = path(tool).unwrap();
        let segments = segments(&std::fs::read(&program).unwrap());
        assert!(!segments.is_empty(), "{tool} is not an ELF executable");
        assert!(
            !segments.contains(&PT_INTERP) && !segments.contains(&PT_DYNAMIC),
            "{tool} is dynamically linked"
        );
        let mode = std::fs::metadata(&program).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "{tool}");
        let output = Command::new(&program)
            .env_clear()
            .arg(flag)
            .output()
            .unwrap();
        let text =
            String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
        assert!(text.contains(says), "{tool} {flag}: {text}");
    }
}

const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;

/// The kinds of the segments a 64-bit little-endian ELF file has. A static executable has neither
/// a program interpreter (the dynamic loader) nor a dynamic section, as readelf -l shows.
fn segments(elf: &[u8]) -> Vec<u32> {
    if elf.len() < 64 || &elf[..5] != b"\x7fELF\x02" {
        return vec![];
    }
    let at = |offset: usize, len: usize| {
        elf[offset..offset + len]
            .iter()
            .rev()
            .fold(0usize, |n, b| n << 8 | *b as usize)
    };
    let (table, size, count) = (at(0x20, 8), at(0x36, 2), at(0x38, 2));
    (0..count).map(|i| at(table + i * size, 4) as u32).collect()
}

/// The firmware qemu loads sits beside it, where `-L` points.
#[test]
fn firmware_is_beside_qemu() {
    let dir = path("").unwrap();
    for firmware in [
        "bios-256k.bin",
        "kvmvapic.bin",
        "vgabios-stdvga.bin",
        "efi-virtio.rom",
        "edk2-aarch64-code.fd",
    ] {
        assert!(
            dir.join(firmware).metadata().unwrap().len() > 0,
            "{firmware}"
        );
    }
}
