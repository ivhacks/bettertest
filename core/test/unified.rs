// End to end: the real bettertest binary runs `unified`, each in a container of its own, so its db,
// its ports and the containers its worker sweeps away at startup are that container's, never this
// machine's or another test's. Cargo sets CARGO_BIN_EXE only for an integration test, which is why
// this file is one (`[[test]]` in core/Cargo.toml), though it lives in test/ beside the others.

use bettertest_shared::{Run, RunState, Worker};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// debian:trixie with python3, which the boss parses pipedefs with and the worker runs tasks in.
const IMAGE: &str = "bettertest-test-python";
const LOCAL_WORKER: &str = "http://[::1]:7358";

fn arch_label() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm"
    } else {
        "x86"
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn image_ready() -> bool {
    Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn build_image() {
    // Three tests build this at once. Docker's tag of one name races: the loser exits with
    // "already exists" while the image is not visible yet. Wait for the one that won.
    for _ in 0..40 {
        if image_ready() {
            return;
        }
        let mut build = Command::new("docker")
            .args(["build", "--quiet", "--tag", IMAGE, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // curl is what the homepage's one-line command uses, and what that script uses to fetch the binary.
        let dockerfile = b"FROM debian:trixie\nRUN apt-get update && apt-get install -y --no-install-recommends python3 curl && rm -rf /var/lib/apt/lists/*\n";
        build.stdin.take().unwrap().write_all(dockerfile).unwrap();
        let out = build.wait_with_output().unwrap();
        if out.status.success() || image_ready() {
            return;
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.contains("already exists") {
            panic!("could not build {IMAGE}: {err}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("could not build {IMAGE}");
}

/// A pipedef whose one task asks only for this machine's architecture label, in a fresh directory.
fn pipedef_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bettertest-unified-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let source = r#"
from bettertest import Labels, Stage, task


class Label(Labels):
    LABEL = "LABEL"


class Hello(Stage):
    labels = (Label.LABEL,)

    @task()
    def world():
        print("hello, world")
"#
    .replace("LABEL", arch_label());
    fs::write(dir.join("hello.py"), source).unwrap();
    dir
}

/// A docker network of its own, so containers reach each other by name.
struct Network(String);

impl Network {
    fn new() -> Self {
        let name = format!("bettertest-unified-{}", uuid::Uuid::new_v4());
        docker(&["network", "create", &name]);
        Network(name)
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["network", "rm", &self.0])
            .output();
    }
}

/// `bettertest <args>` in a container, removed when dropped.
struct Container(String);

impl Container {
    fn start(network: &Network, alias: &str, pipedefs: &PathBuf, args: &[&str]) -> Self {
        let name = format!("bettertest-unified-{}", uuid::Uuid::new_v4());
        let binary = format!(
            "{}:/usr/bin/bettertest:ro",
            env!("CARGO_BIN_EXE_bettertest")
        );
        let pipedefs = format!("{}:/pipedefs:ro,z", pipedefs.display());
        let mut run = vec![
            "run",
            "--detach",
            "--init",
            "--name",
            &name,
            "--network",
            &network.0,
            "--network-alias",
            alias,
            // The worker listens on [::], and docker turns IPv6 off in a container by default.
            "--sysctl",
            "net.ipv6.conf.all.disable_ipv6=0",
            "--publish",
            "127.0.0.1::7357",
            "--volume",
            &binary,
            "--volume",
            &pipedefs,
            IMAGE,
            "bettertest",
        ];
        run.extend_from_slice(args);
        docker(&run);
        Container(name)
    }

    /// The boss, as this machine reaches it. A restart may move it.
    fn boss(&self) -> String {
        let port = docker(&["port", &self.0, "7357/tcp"]);
        let port = port.lines().next().unwrap_or_default();
        format!("http://{port}")
    }

    fn logs(&self) -> String {
        let out = Command::new("docker")
            .args(["logs", &self.0])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
    }

    fn restart(&self) {
        docker(&["restart", &self.0]);
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.0])
            .output();
    }
}

/// The workers the boss lists once `ready` holds, waiting up to 30 s for it: the boss takes a
/// moment to start, and a worker's `worker` event a moment to arrive.
async fn workers_once(container: &Container, ready: impl Fn(&[Worker]) -> bool) -> Vec<Worker> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while Instant::now() < deadline {
        let reply = reqwest::get(format!("{}/api/workers", container.boss())).await;
        match reply {
            Ok(reply) => {
                let text = reply.text().await.unwrap_or_default();
                if let Ok(workers) = serde_json::from_str::<Vec<Worker>>(&text)
                    && ready(&workers)
                {
                    return workers;
                }
                last = text;
            }
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("workers never ready; last: {last}\n{}", container.logs());
}

fn up_with_label(worker: &Worker) -> bool {
    let labels = worker.info.as_ref().map(|info| info.labels.as_slice());
    worker.problem.is_none() && labels.unwrap_or_default().iter().any(|l| l == arch_label())
}

async fn add_worker(container: &Container, url: &str) {
    let reply = reqwest::Client::new()
        .post(format!("{}/api/add-worker", container.boss()))
        .json(&serde_json::json!({ "url": url }))
        .send()
        .await
        .unwrap();
    assert!(
        reply.status().is_success(),
        "{reply:?}\n{}",
        container.logs()
    );
}

#[tokio::test]
async fn unified_alone_enrolls_its_worker_and_runs() {
    build_image();
    let pipedefs = pipedef_dir();
    let network = Network::new();
    let unified = Container::start(&network, "unified", &pipedefs, &["unified", "/pipedefs"]);

    let workers = workers_once(&unified, |w| w.len() == 1 && up_with_label(&w[0])).await;
    assert_eq!(workers[0].url, LOCAL_WORKER);

    // A pipeline that asks only for a label runs, on that worker, with nothing enrolled by hand.
    let client = reqwest::Client::new();
    let reply = client
        .post(format!("{}/api/start-run/hello", unified.boss()))
        .send()
        .await
        .unwrap();
    assert!(reply.status().is_success(), "{reply:?}\n{}", unified.logs());
    let run: Run = reply.json().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        let url = format!("{}/api/run/{}", unified.boss(), run.id);
        let now: Run = client.get(url).send().await.unwrap().json().await.unwrap();
        if now.state != RunState::Running {
            break now;
        }
        assert!(
            Instant::now() < deadline,
            "run still going\n{}",
            unified.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let task = &run.stages[0].tasks[0];
    let url = format!("{}/api/task/{}/log", unified.boss(), task.id);
    let log = client.get(url).send().await.unwrap().text().await.unwrap();
    assert_eq!(run.state, RunState::Pass, "{log}\n{}", unified.logs());
    assert_eq!(task.worker.as_deref(), Some(LOCAL_WORKER), "{log}");
    assert!(log.contains("hello, world"), "{log}");

    // Adding the url it already enrolled leaves one row.
    add_worker(&unified, LOCAL_WORKER).await;
    let workers = workers_once(&unified, |_| true).await;
    assert_eq!(workers.len(), 1, "{workers:?}");

    // So does a restart, which reads that row back and enrolls the same url again.
    unified.restart();
    let workers = workers_once(&unified, |w| w.iter().any(up_with_label)).await;
    assert_eq!(workers.len(), 1, "{workers:?}");
    assert_eq!(workers[0].url, LOCAL_WORKER);
    let _ = fs::remove_dir_all(pipedefs);
}

#[tokio::test]
async fn unified_plus_one_worker_lists_both() {
    build_image();
    let pipedefs = pipedef_dir();
    let network = Network::new();
    let unified = Container::start(&network, "unified", &pipedefs, &["unified", "/pipedefs"]);
    let _extra = Container::start(&network, "extra", &pipedefs, &["worker"]);
    workers_once(&unified, |w| w.len() == 1).await;

    add_worker(&unified, "http://extra:7358").await;
    let workers = workers_once(&unified, |w| w.len() == 2 && w.iter().all(up_with_label)).await;
    let mut urls: Vec<&str> = workers.iter().map(|w| w.url.as_str()).collect();
    urls.sort();
    assert_eq!(urls, ["http://[::1]:7358", "http://extra:7358"]);
    let _ = fs::remove_dir_all(pipedefs);
}

fn machine_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

/// A container whose command is a shell script, not the mounted binary: the machine the homepage's
/// command lands on.
fn run_script(network: &Network, command: &str) -> Container {
    let name = format!("bettertest-unified-{}", uuid::Uuid::new_v4());
    docker(&[
        "run",
        "--detach",
        "--init",
        "--name",
        &name,
        "--network",
        &network.0,
        "--sysctl",
        "net.ipv6.conf.all.disable_ipv6=0",
        "--publish",
        "127.0.0.1::7357",
        // A terminal, as the command is pasted into. On a pipe, stdout stays buffered and the two
        // links never reach `docker logs`.
        "--tty",
        IMAGE,
        "sh",
        "-c",
        command,
    ]);
    Container(name)
}

async fn http(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while Instant::now() < deadline {
        match reqwest::get(url).await {
            Ok(reply) => return reply,
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("GET {url}: {last}");
}

async fn curl_in(container: &Container, url: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = String::new();
    while Instant::now() < deadline {
        let out = Command::new("docker")
            .args(["exec", &container.0, "curl", "-fsS", url])
            .output()
            .unwrap();
        last = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.success() {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("curl {url} failed: {last}\n{}", container.logs());
}

/// The homepage command, pointed at a boss built from this commit: it downloads, installs, and runs
/// unified with the example pipeline. Both links it prints answer. The other architecture is
/// downloaded and not run.
#[tokio::test]
async fn the_install_command_runs_unified_and_both_links_answer() {
    build_image();
    let native = machine_arch();
    let other = if native == "aarch64" {
        "x86_64"
    } else {
        "aarch64"
    };
    let pipedefs = pipedef_dir();
    let network = Network::new();
    let origin = Container::start(&network, "origin", &pipedefs, &["boss", "/pipedefs"]);
    let base = origin.boss();
    let script = http(&format!("{base}/install.sh?arch={native}"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        script.contains(&format!("{base}/dl/bettertest-{native}")),
        "{script}"
    );
    assert!(
        script.contains(&format!("Label.{}", arch_label())),
        "{script}"
    );
    assert!(script.contains("unified"), "{script}");
    let arm = http(&format!("{base}/install.sh?arch=aarch64"))
        .await
        .text()
        .await
        .unwrap();
    assert!(arm.contains("/dl/bettertest-aarch64"), "{arm}");
    assert!(arm.contains("Label.arm"), "{arm}");
    let bad = http(&format!("{base}/install.sh?arch=nope")).await;
    assert_eq!(bad.status(), 400, "{bad:?}");

    docker(&[
        "exec",
        &origin.0,
        "sh",
        "-c",
        &format!(
            "mkdir -p /var/tmp/bettertest/dl \
             && cp /usr/bin/bettertest /var/tmp/bettertest/dl/bettertest-{native} \
             && printf 'not-the-native-binary\\n' > /var/tmp/bettertest/dl/bettertest-{other}"
        ),
    ]);
    let got = http(&format!("{base}/dl/bettertest-{native}")).await;
    assert!(got.status().is_success(), "{got:?}");
    let bytes = got.bytes().await.unwrap();
    assert!(
        bytes.starts_with(b"\x7fELF"),
        "the download is not a binary"
    );
    let other_bytes = http(&format!("{base}/dl/bettertest-{other}"))
        .await
        .bytes()
        .await
        .unwrap();
    assert_eq!(&other_bytes[..], b"not-the-native-binary\n");

    // The same command the homepage shows, with this boss's host instead of the live one.
    let guest = run_script(
        &network,
        &format!("curl -fsSL 'http://origin:7357/install.sh?arch={native}' | sh"),
    );
    let logs = {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let logs = guest.logs();
            if logs.contains("boss web ui at http://[::1]:7357")
                && logs.contains("worker listening on http://[::1]:7358")
            {
                break logs;
            }
            assert!(
                Instant::now() < deadline,
                "unified never printed both links\n{logs}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    // The two links the binary printed. The boss there is serving the example pipeline (its web
    // page is the release binary's embedded UI; a debug binary reads that UI from the build
    // directory, which this container does not have). The worker answers its health check.
    let pipelines = curl_in(&guest, "http://[::1]:7357/api/pipelines").await;
    assert!(pipelines.contains("hello"), "{pipelines}\n{logs}");
    let health = curl_in(&guest, "http://[::1]:7358/health").await;
    assert!(health.contains("ok"), "{health}\n{logs}");

    let workers = workers_once(&guest, |w| w.iter().any(up_with_label)).await;
    assert_eq!(workers.len(), 1, "{workers:?}\n{}", guest.logs());
    let client = reqwest::Client::new();
    let reply = client
        .post(format!("{}/api/start-run/hello", guest.boss()))
        .send()
        .await
        .unwrap();
    assert!(reply.status().is_success(), "{reply:?}\n{}", guest.logs());
    let run: Run = reply.json().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        let now: Run = client
            .get(format!("{}/api/run/{}", guest.boss(), run.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if now.state != RunState::Running {
            break now;
        }
        assert!(
            Instant::now() < deadline,
            "run still going\n{}",
            guest.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let task = &run.stages[0].tasks[0];
    let log = client
        .get(format!("{}/api/task/{}/log", guest.boss(), task.id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(run.state, RunState::Pass, "{log}\n{}", guest.logs());
    assert!(log.contains("hello, world"), "{log}");
    let _ = fs::remove_dir_all(pipedefs);
}
