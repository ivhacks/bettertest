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

fn build_image() {
    let mut build = Command::new("docker")
        .args(["build", "--quiet", "--tag", IMAGE, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let dockerfile = b"FROM debian:trixie\nRUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*\n";
    build.stdin.take().unwrap().write_all(dockerfile).unwrap();
    assert!(build.wait().unwrap().success(), "could not build {IMAGE}");
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
