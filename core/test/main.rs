// Tests of main.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use clap::Parser;

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
