// Tests of client.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use bettertest_shared::{Artifact, LogStream, Run, Task, Tests};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[test]
fn events_split_across_chunks() {
    let mut decoder = SseDecoder::default();
    assert_eq!(decoder.feed(b"event: stdout\nda"), vec![]);
    assert_eq!(
        decoder.feed(b"ta: hello\n\nevent: exit\ndata: 0\n\n"),
        vec![
            ("stdout".to_string(), "hello".to_string()),
            ("exit".to_string(), "0".to_string()),
        ]
    );
}

struct Memo {
    lines: Vec<String>,
    wait: bool,
    tests: Vec<Tests>,
    run: Run,
    cancel: tokio::sync::watch::Sender<Option<String>>,
}

impl TaskIo for Memo {
    fn log(&mut self, _stream: LogStream, line: String) {
        self.lines.push(line);
    }

    fn outputs(&self) -> Result<PathBuf, String> {
        Ok(PathBuf::from("."))
    }

    fn saved(&mut self, _: Artifact) {}

    fn tests(&mut self, tests: Tests) {
        self.tests.push(tests);
    }

    // A task of a run, when the test plays the boss: the boss waits on a busy worker.
    fn boss(&self) -> Option<FromBoss<'_>> {
        self.wait.then(|| FromBoss {
            run: &self.run,
            name: Some("boss.example"),
            port: 7357,
            token: Some("t0ken"),
            cancelled: self.cancel.subscribe(),
        })
    }
}

fn memo(wait: bool) -> Memo {
    Memo {
        lines: vec![],
        wait,
        tests: vec![],
        run: Run {
            id: uuid::Uuid::new_v4(),
            pipeline: "p".to_string(),
            state: bettertest_shared::RunState::Running,
            stages: vec![],
            commit: Some(bettertest_shared::Commit {
                sha: "3fe66e3".to_string(),
                branch: "main".to_string(),
            }),
        },
        cancel: tokio::sync::watch::channel(None).0,
    }
}

fn task() -> Task {
    Task {
        name: "noop".to_string(),
        worker: None,
        labels: vec![],
        image: None,
        vm: None,
        arch: None,
        timeout: None,
        sandbox: None,
    }
}

/// Serves `replies` in order, one HTTP request each, and keeps the requests that arrived.
fn serve(replies: Vec<Vec<u8>>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(vec![]));
    let requests = seen.clone();
    std::thread::spawn(move || {
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&got[..end]);
                    let length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    });
                    if let Some(length) = length
                        && got.len() >= end + 4 + length
                    {
                        break;
                    }
                }
            }
            requests
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&got).into_owned());
            let _ = sock.write_all(&reply);
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn http_reply(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn the_boss_retries_when_the_worker_is_briefly_busy() {
    let busy =
        "this worker is busy: it runs as many tasks at once as it was started to (worker --tasks)";
    let (url, seen) = serve(vec![
        http_reply("409 Conflict", busy),
        http_reply("200 OK", "event: exit\ndata: 0\n\n"),
    ]);
    let mut io = memo(true);
    let passed = run_task(
        &http().unwrap(),
        &url,
        "",
        "T",
        &task(),
        &Target::None,
        &mut io,
    )
    .await;
    assert!(passed, "{:?}", io.lines);
    assert!(
        io.lines
            .iter()
            .any(|line| line.contains("waiting:") && line.contains(busy)),
        "{:?}",
        io.lines
    );
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn dispatch_does_not_retry_a_busy_worker() {
    let busy =
        "this worker is busy: it runs as many tasks at once as it was started to (worker --tasks)";
    let (url, seen) = serve(vec![http_reply("409 Conflict", busy)]);
    let mut io = memo(false);
    let passed = run_task(
        &http().unwrap(),
        &url,
        "",
        "T",
        &task(),
        &Target::None,
        &mut io,
    )
    .await;
    assert!(!passed);
    assert!(
        io.lines.iter().any(|line| line.contains(busy)),
        "{:?}",
        io.lines
    );
    assert!(
        !io.lines.iter().any(|line| line.starts_with("waiting:")),
        "{:?}",
        io.lines
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn multibyte_character_split_across_chunks() {
    let mut decoder = SseDecoder::default();
    let bytes = "event: stdout\ndata: 🤘\n\n".as_bytes();
    let middle_of_emoji = bytes.len() - 4;
    assert_eq!(decoder.feed(&bytes[..middle_of_emoji]), vec![]);
    assert_eq!(
        decoder.feed(&bytes[middle_of_emoji..]),
        vec![("stdout".to_string(), "🤘".to_string())]
    );
}

#[tokio::test]
async fn test_counts_are_logged_and_kept() {
    let (url, _) = serve(vec![http_reply(
        "200 OK",
        "event: stdout\ndata: 2 errors out of 1234 tests\n\nevent: tests\ndata: {\"total\":1234,\"failed\":2}\n\nevent: exit\ndata: 1\n\n",
    )]);
    let mut io = memo(false);
    let passed = run_task(
        &http().unwrap(),
        &url,
        "",
        "T",
        &task(),
        &Target::None,
        &mut io,
    )
    .await;
    assert!(!passed);
    assert_eq!(
        io.tests,
        [Tests {
            total: 1234,
            failed: 2
        }]
    );
    assert_eq!(
        io.lines[..2],
        ["2 errors out of 1234 tests", "1234 tests, 2 failed"]
    );
}

/// The fields of the multipart form in `request`, in order.
fn form_fields(request: &str) -> Vec<(String, String)> {
    let parts = request.split("form-data; name=\"").skip(1);
    parts
        .filter_map(|part| {
            let (name, rest) = part.split_once('"')?;
            let (_, value) = rest.split_once("\r\n\r\n")?;
            let (value, _) = value.split_once("\r\n--")?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

#[tokio::test]
async fn a_task_of_a_run_tells_the_worker_about_its_boss_and_dispatch_does_not() {
    let exit = || http_reply("200 OK", "event: exit\ndata: 0\n\n");
    let (url, seen) = serve(vec![exit(), exit()]);
    let mut of_run = memo(true);
    let run_id = of_run.run.id.to_string();
    let target = Target::Container("c1".to_string());
    assert!(
        run_task(
            &http().unwrap(),
            &url,
            "P",
            "T",
            &task(),
            &target,
            &mut of_run
        )
        .await
    );
    let mut dispatched = memo(false);
    assert!(
        run_task(
            &http().unwrap(),
            &url,
            "P",
            "T",
            &task(),
            &Target::None,
            &mut dispatched
        )
        .await
    );
    let seen = seen.lock().unwrap();
    let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
    let mut sent = form_fields(&seen[0]);
    sent.sort();
    let mut expected = vec![
        pair("boss_name", "boss.example"),
        pair("boss_port", "7357"),
        pair("boss_token", "t0ken"),
        pair("commit", "3fe66e3"),
        pair("pipedef", "P"),
        pair("pipeline", "p"),
        pair("run", &run_id),
        pair("stage", "T"),
        pair("target", "container:c1"),
        pair("task", "noop"),
    ];
    expected.sort();
    assert_eq!(sent, expected);
    let dispatch = vec![
        pair("pipedef", "P"),
        pair("stage", "T"),
        pair("task", "noop"),
    ];
    assert_eq!(form_fields(&seen[1]), dispatch);
}
