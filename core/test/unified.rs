// End to end: starts the real bettertest binary. Cargo sets CARGO_BIN_EXE only for an integration
// test, which is why this file is one (`[[test]]` in core/Cargo.toml) rather than a child module.
// It lives in test/ beside the others. Each run gets its own --workdir and ports, so it never
// touches /var/tmp/bettertest or another test's boss.

use bettertest_shared::{Run, RunState, Worker};
use std::{
    fs,
    io::{self, ErrorKind, Read, Write},
    net::{Ipv6Addr, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn arch_label() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm"
    } else {
        "x86"
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("bettertest-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A bettertest process, killed (and its children with it) when dropped.
struct Proc {
    child: Option<Child>,
    log: PathBuf,
}

impl Proc {
    fn start(args: &[String], log: PathBuf) -> Self {
        let file = fs::File::create(&log).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_bettertest"));
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(file.try_clone().unwrap()))
            .stderr(Stdio::from(file))
            // Startup sweeps every container labeled bettertest.worker. The in-process worker
            // tests create those. A socket that is not there makes the sweep fail at once,
            // instead of deleting theirs. Nothing in these tests runs a container.
            .env(
                "DOCKER_HOST",
                "unix:///tmp/bettertest-unified-test-no-docker.sock",
            )
            .process_group(0);
        let child = command.spawn().unwrap();
        Self {
            child: Some(child),
            log,
        }
    }

    fn exited(&mut self) -> Option<String> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => Some(format!("exited {status}\n{}", self.output())),
            _ => None,
        }
    }

    fn output(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let id = child.id();
        let _ = Command::new("sh")
            .args(["-c", &format!("kill -KILL -{id}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pipedef(dir: &Path, label: &str) -> PathBuf {
    let path = dir.join("hello.py");
    let source = r#"from bettertest import Labels, Stage, task


class Label(Labels):
    LABEL = "LABEL"


class Hello(Stage):
    labels = (Label.LABEL,)

    @task()
    def world():
        print("hello, world")
"#
    .replace("LABEL", label);
    fs::write(&path, source).unwrap();
    path
}

fn unified_args(work: &Path, pipedef: &Path, boss: u16, worker: u16) -> Vec<String> {
    vec![
        "unified".into(),
        "--workdir".into(),
        work.display().to_string(),
        "--boss-port".into(),
        boss.to_string(),
        "--worker-port".into(),
        worker.to_string(),
        pipedef.display().to_string(),
    ]
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap().port()
}

struct Reply {
    status: u16,
    body: String,
}

fn http(method: &str, port: u16, path: &str, body: Option<&[u8]>) -> io::Result<Reply> {
    let body = body.unwrap_or(b"");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: [::1]:{port}\r\nConnection: close\r\nAccept: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str("\r\n");
    let mut request = head.into_bytes();
    request.extend_from_slice(body);

    let mut stream = TcpStream::connect((Ipv6Addr::LOCALHOST, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(&request)?;

    let mut raw = Vec::new();
    let mut buf = [0; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::WouldBlock => {
                if response_complete(&raw) {
                    break;
                }
                if raw.is_empty() {
                    return Err(e);
                }
                return Err(io::Error::new(
                    e.kind(),
                    format!("incomplete response: {}", String::from_utf8_lossy(&raw)),
                ));
            }
            Err(e) => return Err(e),
        }
        if response_complete(&raw) {
            break;
        }
    }
    Ok(split_response(&raw))
}

fn response_complete(raw: &[u8]) -> bool {
    let Some((head, body)) = split_head(raw) else {
        return false;
    };
    let head = String::from_utf8_lossy(head);
    if let Some(len) = header_value(&head, "content-length").and_then(|v| v.parse::<usize>().ok()) {
        return body.len() >= len;
    }
    if header_value(&head, "transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        return decode_chunked(body).is_some();
    }
    false
}

fn split_head(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    raw.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (&raw[..i], &raw[i + 4..]))
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn decode_chunked(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let end = body.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&body[..end]).ok()?;
        let size = size.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size, 16).ok()?;
        body = &body[end + 2..];
        if size == 0 {
            return Some(out);
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            return None;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

fn split_response(raw: &[u8]) -> Reply {
    let Some((head, body)) = split_head(raw) else {
        return Reply {
            status: 0,
            body: String::from_utf8_lossy(raw).into_owned(),
        };
    };
    let head = String::from_utf8_lossy(head);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = if header_value(&head, "transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        decode_chunked(body)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    } else if let Some(len) = header_value(&head, "content-length").and_then(|v| v.parse().ok()) {
        let n = body.len().min(len);
        String::from_utf8_lossy(&body[..n]).into_owned()
    } else {
        String::from_utf8_lossy(body).into_owned()
    };
    Reply { status, body }
}

fn workers(port: u16) -> Result<Vec<Worker>, String> {
    let reply = http("GET", port, "/api/workers", None).map_err(|e| e.to_string())?;
    if reply.status != 200 {
        return Err(format!("GET /api/workers {}: {}", reply.status, reply.body));
    }
    serde_json::from_str(&reply.body).map_err(|e| format!("{e}: {}", reply.body))
}

/// Polls until `ready` or the process dies. The last answer is what the assertion shows.
fn wait_workers(
    proc: &mut Proc,
    port: u16,
    ready: impl Fn(&[Worker]) -> bool,
) -> Result<Vec<Worker>, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last;
    loop {
        if let Some(gone) = proc.exited() {
            return Err(gone);
        }
        last = match workers(port) {
            Ok(list) if ready(&list) => return Ok(list),
            Ok(list) => format!("{list:?}"),
            Err(e) => e,
        };
        if Instant::now() >= deadline {
            return Err(format!("{last}\n{}", proc.output()));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn has_label(worker: &Worker, label: &str) -> bool {
    worker
        .info
        .as_ref()
        .is_some_and(|info| info.labels.len() == 1 && info.labels[0] == label)
}

fn task_log(port: u16, id: &str) -> String {
    http("GET", port, &format!("/api/task/{id}/log"), None)
        .map(|reply| reply.body)
        .unwrap_or_default()
}

#[test]
fn unified_alone_enrolls_its_worker_and_runs() {
    let label = arch_label();
    let scratch = Scratch::new("unified");
    let work = scratch.path().join("work");
    fs::create_dir(&work).unwrap();
    let pipedef = pipedef(scratch.path(), label);
    let boss = free_port();
    let worker_port = free_port();
    let url = format!("http://[::1]:{worker_port}");
    let mut proc = Proc::start(
        &unified_args(&work, &pipedef, boss, worker_port),
        scratch.path().join("unified.log"),
    );

    let list = wait_workers(&mut proc, boss, |workers| {
        workers.len() == 1 && workers[0].url == url && has_label(&workers[0], label)
    })
    .unwrap_or_else(|e| panic!("expected one {label} worker at {url}: {e}"));
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].url, url);
    assert!(has_label(&list[0], label), "{list:?}");
    assert!(
        work.join("bettertest.db").is_file(),
        "the db is not in --workdir {}\n{}",
        work.display(),
        proc.output()
    );

    let reply = http("POST", boss, "/api/start-run/hello", None).unwrap_or_else(|e| {
        panic!("start-run: {e}\n{}", proc.output());
    });
    assert_eq!(reply.status, 200, "{}\n{}", reply.body, proc.output());
    let started: Run = serde_json::from_str(&reply.body).unwrap_or_else(|e| {
        panic!("{e}: {}\n{}", reply.body, proc.output());
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let finished = loop {
        if let Some(gone) = proc.exited() {
            panic!("unified died while the run was going\n{gone}");
        }
        match http("GET", boss, &format!("/api/run/{}", started.id), None) {
            Ok(reply) if reply.status == 200 => {
                let run: Run = serde_json::from_str(&reply.body).unwrap_or_else(|e| {
                    panic!("{e}: {}\n{}", reply.body, proc.output());
                });
                if run.state != RunState::Running {
                    break run;
                }
            }
            Ok(reply) => panic!(
                "GET run {}: {}\n{}",
                reply.status,
                reply.body,
                proc.output()
            ),
            Err(e) if Instant::now() >= deadline => {
                panic!("run did not finish: {e}\n{}", proc.output())
            }
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            panic!("run still going\n{}", proc.output());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let task = &finished.stages[0].tasks[0];
    let log = task_log(boss, &task.id.to_string());
    assert_eq!(
        finished.state,
        RunState::Pass,
        "{log}\n{finished:?}\n{}",
        proc.output()
    );
    assert_eq!(task.worker.as_deref(), Some(url.as_str()), "{log}");
    assert!(log.contains("hello, world"), "{log}");

    // Adding the url unified already enrolled must not create a second row.
    let add = format!(r#"{{"url":"{url}"}}"#);
    let reply = http("POST", boss, "/api/add-worker", Some(add.as_bytes())).unwrap();
    assert_eq!(reply.status, 204, "{}\n{}", reply.body, proc.output());
    let list = workers(boss).unwrap();
    assert_eq!(
        list.len(),
        1,
        "add-worker duplicated the local worker: {list:?}"
    );
    assert_eq!(list[0].url, url);

    // A restart reads that one row back and enrolls it again. Still one.
    proc.stop();
    let mut proc = Proc::start(
        &unified_args(&work, &pipedef, boss, worker_port),
        scratch.path().join("unified-again.log"),
    );
    let list = wait_workers(&mut proc, boss, |workers| {
        workers.len() == 1 && workers[0].url == url && has_label(&workers[0], label)
    })
    .unwrap_or_else(|e| panic!("after restart, expected one {label} worker at {url}: {e}"));
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].url, url);
}

#[test]
fn unified_plus_one_worker_lists_both() {
    let scratch = Scratch::new("both");
    let unified_work = scratch.path().join("unified");
    let worker_work = scratch.path().join("worker");
    fs::create_dir(&unified_work).unwrap();
    fs::create_dir(&worker_work).unwrap();
    let pipedef = pipedef(scratch.path(), arch_label());
    let boss = free_port();
    let unified_port = free_port();
    let extra_port = free_port();
    let unified_url = format!("http://[::1]:{unified_port}");
    let extra_url = format!("http://[::1]:{extra_port}");

    let mut unified = Proc::start(
        &unified_args(&unified_work, &pipedef, boss, unified_port),
        scratch.path().join("unified.log"),
    );
    let mut extra = Proc::start(
        &[
            "worker".into(),
            "--workdir".into(),
            worker_work.display().to_string(),
            "--worker-port".into(),
            extra_port.to_string(),
        ],
        scratch.path().join("worker.log"),
    );

    // The API is up. On a boss that enrolled its own worker, the list already has it.
    wait_workers(&mut unified, boss, |_| true)
        .unwrap_or_else(|e| panic!("boss did not come up: {e}"));
    if let Some(gone) = extra.exited() {
        panic!("worker died\n{gone}");
    }

    let add = format!(r#"{{"url":"{extra_url}"}}"#);
    let reply = http("POST", boss, "/api/add-worker", Some(add.as_bytes()))
        .unwrap_or_else(|e| panic!("add-worker: {e}\n{}", unified.output()));
    assert_eq!(reply.status, 204, "{}\n{}", reply.body, unified.output());

    let list = workers(boss).unwrap_or_else(|e| panic!("{e}\n{}", unified.output()));
    let mut urls: Vec<String> = list.iter().map(|w| w.url.clone()).collect();
    urls.sort();
    let mut want = vec![unified_url, extra_url];
    want.sort();
    assert_eq!(
        urls,
        want,
        "expected the unified worker and the one added by url\n{}\n{}",
        unified.output(),
        extra.output()
    );
    assert!(
        unified_work.join("bettertest.db").is_file(),
        "unified db is not in its --workdir"
    );
    assert!(
        worker_work.join("tasks").is_dir(),
        "the dedicated worker did not use its --workdir"
    );
}
