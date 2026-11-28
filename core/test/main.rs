// Tests of main.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use clap::Parser;

/// Docker is the one thing bettertest's tests need from the machine. A test that uses it calls
/// this first, so that without it the test fails saying so, not on whatever came back instead.
pub fn require_docker() {
    let info = std::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output();
    let why = match info {
        Ok(out) if out.status.success() => return,
        Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
        Err(e) => format!("could not run docker: {e}"),
    };
    panic!(
        "this test needs docker, the one thing the tests need from the machine: install it and \
         start its daemon, as the cloud VM's start script does. docker info said: {why}"
    );
}

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

/// A task still going when its command ends keeps a runtime that works until the process is gone.
/// The prober waits until tokio would refuse blocking work, as a runtime being torn down does, and
/// only then lets the task ask for some: a boss going on with a run once its worker said it was
/// restarting read the pipedef just then, got "background task failed", and failed the run.
#[test]
fn a_task_still_going_as_the_command_ends_is_not_refused() {
    use std::sync::{Arc, atomic::*, mpsc};
    let (go, wait) = mpsc::channel();
    let (read, result) = mpsc::channel();
    let (handle, runtime) = mpsc::channel();
    let ended = Arc::new(AtomicBool::new(false));
    let over = ended.clone();
    let prober = std::thread::spawn(move || {
        let runtime: tokio::runtime::Handle = runtime.recv().unwrap();
        loop {
            let ran = Arc::new(AtomicBool::new(false));
            let probe = runtime.spawn_blocking({
                let ran = ran.clone();
                move || ran.store(true, Ordering::SeqCst)
            });
            std::thread::sleep(std::time::Duration::from_millis(10));
            let refused = probe.is_finished() && !ran.load(Ordering::SeqCst);
            if refused || over.load(Ordering::SeqCst) {
                return go.send(()).unwrap();
            }
        }
    });
    let command = async move {
        let (started, holding) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            // Holds a worker thread, as a busy one would, until the prober says.
            started.send(()).unwrap();
            wait.recv().unwrap();
            let metadata = tokio::fs::metadata("/").await;
            read.send(metadata.map(|_| ()).map_err(|e| e.to_string())).unwrap();
        });
        holding.await.unwrap();
        handle.send(tokio::runtime::Handle::current()).unwrap();
        Ok(())
    };
    block_on(command).unwrap();
    ended.store(true, Ordering::SeqCst);
    prober.join().unwrap();
    let timeout = std::time::Duration::from_secs(10);
    assert_eq!(result.recv_timeout(timeout), Ok(Ok(())));
}
