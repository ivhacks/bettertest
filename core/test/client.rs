// Tests of client.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use bettertest_shared::{Artifact, LogStream, Run, Task};
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
}

impl TaskIo for Memo {
    fn log(&mut self, _stream: LogStream, line: String) {
        self.lines.push(line);
    }

    fn outputs(&self) -> Result<PathBuf, String> {
        Ok(PathBuf::from("."))
    }

    fn saved(&mut self, _: Artifact) {}

    fn run(&self) -> Option<&Run> {
        None
    }

    fn boss_name(&self) -> Option<&str> {
        None
    }

    fn boss_port(&self) -> u16 {
        7357
    }

    fn cancelled(&self) -> Option<tokio::sync::watch::Receiver<Option<String>>> {
        None
    }

    fn boss_token(&self) -> Option<&str> {
        None
    }

    fn wait_when_busy(&self) -> bool {
        self.wait
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

/// Serves `replies` in order, one HTTP request each, and counts how many arrived.
fn serve(replies: Vec<Vec<u8>>) -> (String, Arc<Mutex<usize>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(0));
    let count = seen.clone();
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
            *count.lock().unwrap() += 1;
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
    let mut io = Memo {
        lines: vec![],
        wait: true,
    };
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
    assert_eq!(*seen.lock().unwrap(), 2);
}

#[tokio::test]
async fn dispatch_does_not_retry_a_busy_worker() {
    let busy =
        "this worker is busy: it runs as many tasks at once as it was started to (worker --tasks)";
    let (url, seen) = serve(vec![http_reply("409 Conflict", busy)]);
    let mut io = Memo {
        lines: vec![],
        wait: false,
    };
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
    assert_eq!(*seen.lock().unwrap(), 1);
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
