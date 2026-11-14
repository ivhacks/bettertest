// Tests of main.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use clap::Parser;
use std::path::Path;

fn parse(args: &[&str]) -> Command {
    Args::try_parse_from(args).unwrap().command
}

#[test]
fn boss_port_defaults_to_7357() {
    let Command::Boss { boss_port, .. } = parse(&["bettertest", "boss", "app.py"]) else {
        panic!("expected boss");
    };
    assert_eq!(boss_port, 7357);
}

#[test]
fn worker_port_defaults_to_7358() {
    let Command::Worker { worker_port, .. } = parse(&["bettertest", "worker"]) else {
        panic!("expected worker");
    };
    assert_eq!(worker_port, 7358);
}

#[test]
fn unified_ports_default() {
    let Command::Unified {
        boss_port,
        worker_port,
        ..
    } = parse(&["bettertest", "unified", "app.py"])
    else {
        panic!("expected unified");
    };
    assert_eq!(boss_port, 7357);
    assert_eq!(worker_port, 7358);
}

#[test]
fn boss_port_flag() {
    let Command::Boss { boss_port, .. } =
        parse(&["bettertest", "boss", "--boss-port", "8000", "app.py"])
    else {
        panic!("expected boss");
    };
    assert_eq!(boss_port, 8000);
}

#[test]
fn worker_port_flag() {
    let Command::Worker { worker_port, .. } =
        parse(&["bettertest", "worker", "--worker-port", "8001"])
    else {
        panic!("expected worker");
    };
    assert_eq!(worker_port, 8001);
}

#[test]
fn unified_takes_both_ports() {
    let Command::Unified {
        boss_port,
        worker_port,
        ..
    } = parse(&[
        "bettertest",
        "unified",
        "--boss-port",
        "8000",
        "--worker-port",
        "8001",
        "app.py",
    ])
    else {
        panic!("expected unified");
    };
    assert_eq!(boss_port, 8000);
    assert_eq!(worker_port, 8001);
}

#[test]
fn workdir_flag() {
    let Command::Boss { workdir, .. } =
        parse(&["bettertest", "boss", "--workdir", "/tmp/bt", "app.py"])
    else {
        panic!("expected boss");
    };
    assert_eq!(workdir.workdir.as_deref(), Some(Path::new("/tmp/bt")));

    let Command::Worker { workdir, .. } = parse(&["bettertest", "worker", "--workdir", "/tmp/btw"])
    else {
        panic!("expected worker");
    };
    assert_eq!(workdir.workdir.as_deref(), Some(Path::new("/tmp/btw")));

    let Command::Unified { workdir, .. } =
        parse(&["bettertest", "unified", "--workdir", "/tmp/btu", "app.py"])
    else {
        panic!("expected unified");
    };
    assert_eq!(workdir.workdir.as_deref(), Some(Path::new("/tmp/btu")));
}

#[test]
fn port_zero_is_refused() {
    assert!(Args::try_parse_from(["bettertest", "boss", "--boss-port", "0", "app.py"]).is_err());
    assert!(Args::try_parse_from(["bettertest", "worker", "--worker-port", "0"]).is_err());
}

unsafe extern "C" {
    fn dup(fd: i32) -> i32;
    fn dup2(from: i32, to: i32) -> i32;
}

/// Run by `printing_to_a_full_disk_does_not_panic`, in a process of its own, since it points this
/// process's stdout and stderr at /dev/full for a moment.
#[test]
#[ignore]
fn print_to_dev_full() {
    use std::os::fd::AsRawFd;
    let full = std::fs::File::options()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let (stdout, stderr) = unsafe { (dup(1), dup(2)) };
    unsafe {
        dup2(full.as_raw_fd(), 1);
        dup2(full.as_raw_fd(), 2);
    }
    println!("a line the disk has no room for");
    eprintln!("nor this one");
    unsafe {
        dup2(stdout, 1);
        dup2(stderr, 2);
    }
}

#[test]
fn printing_to_a_full_disk_does_not_panic() {
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "tests::print_to_dev_full",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .unwrap();
    let output = String::from_utf8_lossy(&child.stdout);
    assert!(child.status.success(), "{output}");
    assert!(output.contains("1 passed"), "{output}");
}
