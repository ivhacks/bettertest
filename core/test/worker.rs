// Tests of worker.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

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

const PIPEDEF: &str = "
from bettertest import Stage, task

class T(Stage):
    @task()
    def noop():
        pass

    @task()
    def sleep():
        import time
        time.sleep(30)
";

fn task_form(name: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("pipedef", PIPEDEF.to_string())
        .text("stage", "T")
        .text("task", name.to_string())
}

async fn worker(capacity: usize) -> String {
    let shared = Shared {
        vms: Arc::new(vm::Vms::new(1)),
        slots: Arc::new(Semaphore::new(capacity)),
    };
    let app = router(shared, vec!["x86".into()], capacity as u32);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let service = app.into_make_service_with_connect_info::<SocketAddr>();
        axum::serve(listener, service).await.unwrap();
    });
    format!("http://{addr}")
}

async fn until_exit(response: &mut reqwest::Response) -> String {
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk())
            .await
            .unwrap()
            .unwrap();
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        if buf.windows(4).any(|w| w == b"exit") {
            return String::from_utf8_lossy(&buf).into_owned();
        }
    }
    panic!("no exit in {}", String::from_utf8_lossy(&buf));
}

#[tokio::test]
async fn a_running_task_still_fills_the_worker() {
    let url = worker(1).await;
    let client = reqwest::Client::new();
    let first = client
        .post(format!("{url}/start-task"))
        .multipart(task_form("sleep"))
        .send()
        .await
        .unwrap();
    assert!(first.status().is_success(), "{}", first.status());
    let second = client
        .post(format!("{url}/start-task"))
        .multipart(task_form("noop"))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let body = second.text().await.unwrap();
    assert!(body.contains("this worker is busy"), "{body}");
}

#[tokio::test]
async fn a_finished_task_frees_its_slot_before_the_caller_hangs_up() {
    let url = worker(1).await;
    let client = reqwest::Client::new();
    let mut first = client
        .post(format!("{url}/start-task"))
        .multipart(task_form("noop"))
        .send()
        .await
        .unwrap();
    assert!(first.status().is_success(), "{}", first.status());
    let log = until_exit(&mut first).await;
    assert!(log.contains("exit"), "{log}");
    // The first response is still open, which is when the boss learns the task exited and
    // starts the next one. The slot has to be free already.
    let second = client
        .post(format!("{url}/start-task"))
        .multipart(task_form("noop"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::OK,
        "{}",
        second.text().await.unwrap()
    );
    drop(first);
}

#[test]
fn reboot_says_why_the_worker_refused() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let _ = sock.read(&mut buf);
        let body = "vm abc did not come back within 1800s\n\
                    last lines of the vm console:\n\
                    emergency mode";
        let header = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(header.as_bytes()).unwrap();
        sock.write_all(body.as_bytes()).unwrap();
    });
    let script = format!(
        r#"
import bettertest
bettertest._target = {{"kind": "vm", "reboot": "http://127.0.0.1:{port}/vm/abc/reboot"}}
try:
    bettertest.reboot()
except RuntimeError as e:
    text = str(e)
    assert "500" in text, text
    assert "did not come back" in text, text
    assert "emergency mode" in text, text
    assert "HTTP Error" not in text, text
else:
    raise SystemExit("reboot() swallowed the error")
"#
    );
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../pylib/src");
    let out = std::process::Command::new("python3")
        .env("PYTHONPATH", root)
        .arg("-c")
        .arg(&script)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn deleting_a_container_deletes_the_volumes_its_image_declares() {
    let image = "bettertest-test-volume";
    let mut build = std::process::Command::new("docker")
        .args(["build", "--quiet", "--tag", image, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let dockerfile = b"FROM debian:trixie\nVOLUME /work\n";
    build.stdin.take().unwrap().write_all(dockerfile).unwrap();
    assert!(build.wait().unwrap().success());
    let url = worker(1).await;
    let client = reqwest::Client::new();
    let started: Started = client
        .post(format!("{url}/start-container"))
        .json(&serde_json::json!({"image": image}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let format = "{{range .Mounts}}{{.Name}}{{end}}";
    let inspect = std::process::Command::new("docker")
        .args(["inspect", "--format", format, &started.id])
        .output()
        .unwrap();
    let volume = String::from_utf8(inspect.stdout).unwrap();
    let volume = volume.trim();
    let exists = || {
        let inspect = std::process::Command::new("docker")
            .args(["volume", "inspect", volume])
            .output()
            .unwrap();
        inspect.status.success()
    };
    assert!(exists(), "no volume {volume:?}");
    let deleted = client
        .delete(format!("{url}/container/{}", started.id))
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success(), "{}", deleted.status());
    assert!(!exists(), "{volume} outlived its container");
}
