// Tests of worker.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use std::net::{IpAddr, Ipv4Addr};

fn start_task(
    pipeline: Option<&str>,
    run: Option<&str>,
    boss_name: Option<&str>,
    boss_port: Option<u16>,
) -> StartTask {
    StartTask {
        pipedef: String::new(),
        stage: String::new(),
        task: String::new(),
        target: None,
        pipeline: pipeline.map(str::to_string),
        run: run.map(str::to_string),
        commit: None,
        boss_name: boss_name.map(str::to_string),
        boss_token: None,
        boss_port,
    }
}

fn address(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), port)
}

#[test]
fn boss_url_uses_the_port_from_the_start_task_address() {
    let run = "01234567-89ab-cdef-0123-456789abcdef";
    let req = start_task(Some("app"), Some(run), None, Some(8000));
    let url = boss_url(&req, address(8000)).unwrap();
    assert_eq!(url.as_deref(), Some("http://192.0.2.1:8000"));
}

#[test]
fn https_boss_url_uses_the_port_from_the_start_task_address() {
    let run = "01234567-89ab-cdef-0123-456789abcdef";
    let req = start_task(Some("app"), Some(run), Some("ci.example"), Some(8443));
    let url = boss_url(&req, address(8443)).unwrap();
    assert_eq!(url.as_deref(), Some("https://ci.example:8443"));
}

#[test]
fn dispatch_has_no_boss_url() {
    let req = start_task(None, None, None, None);
    assert_eq!(boss_url(&req, address(7357)).unwrap(), None);
}
