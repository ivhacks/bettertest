// Tests of boss/runs.rs: stages whose tasks share a sandbox, a pipeline's sandboxes that live across
// stages, and runs a forge's pushes start, against stand-in workers and a stand-in forge. A child of
// the boss's tests (see the #[path] in test/boss.rs), so it reaches what the boss keeps private.

use super::*;
use axum::extract::Multipart;
use serde_json::{Value, json};
use std::{path::Path, process::Command};

type Calls = Arc<Mutex<Vec<String>>>;

/// A stand-in worker for sandboxed tasks. It starts containers c1, c2…, answers each start-task with
/// the next of `exits`, and deleting a container with `stop`. `calls` is what it was asked, in order.
async fn sandbox_worker(exits: Vec<i32>, stop: StatusCode) -> (String, Calls) {
    let calls: Calls = Arc::default();
    let exits = Arc::new(Mutex::new(exits.into_iter()));
    let started = Arc::new(AtomicUsize::new(0));
    let (heard, heard_start, heard_stop) = (calls.clone(), calls.clone(), calls.clone());
    let start_container = move || async move {
        let id = format!("c{}", started.fetch_add(1, Ordering::SeqCst) + 1);
        lock(&heard_start).push(format!("start {id}"));
        Json(json!({ "id": id }))
    };
    let start_task = move |mut form: Multipart| async move {
        let (mut task, mut target) = (String::new(), String::new());
        while let Some(field) = form.next_field().await.unwrap() {
            let name = field.name().unwrap_or_default().to_string();
            let value = field.text().await.unwrap();
            match name.as_str() {
                "task" => task = value,
                "target" => target = value,
                _ => {}
            }
        }
        lock(&heard).push(format!("{task} in {target}"));
        let code = lock(&exits).next().unwrap_or(0);
        let body = format!("event: exit\ndata: {code}\n\n");
        ([(header::CONTENT_TYPE, "text/event-stream")], body)
    };
    let delete = move |AxumPath(id): AxumPath<String>| async move {
        lock(&heard_stop).push(format!("stop {id}"));
        (stop, "docker rm failed")
    };
    let app = Router::new()
        .route("/start-container", post(start_container))
        .route("/start-task", post(start_task))
        .route("/container/{id}", axum::routing::delete(delete));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, calls)
}

/// Runs `source`, a pipedef whose WORKER is the worker at `url`, to its end. The run as the db has
/// it then.
async fn run_to_end(url: &str, source: &str) -> (Shared, Run) {
    let source = source.replace("WORKER", url);
    let file = std::env::temp_dir().join(format!("sandboxes-{}.py", Uuid::new_v4()));
    fs::write(&file, &source).unwrap();
    let boss = boss(workers(&[(url, &["x86"], 0, 4)]));
    lock(&boss.pipedefs).insert("app".into(), Pipedef::File(file.clone()));
    let (source, pipeline) = runs::load_pipeline(&boss, "app", None).await.unwrap();
    let mut events = boss.events.subscribe();
    let loaded = (source, pipeline);
    let run = begin_run(
        &boss,
        &HeaderMap::new(),
        "app",
        loaded,
        None,
        BTreeMap::new(),
    )
    .unwrap();
    finished(&mut events, run.id).await;
    fs::remove_file(&file).unwrap();
    let run = boss.db().run(run.id).unwrap().unwrap();
    (boss, run)
}

fn states(run: &Run) -> Vec<(String, TaskState)> {
    let tasks = run
        .stages
        .iter()
        .flat_map(|s| s.tasks.iter().map(move |t| (s, t)));
    tasks
        .map(|(s, t)| (format!("{}.{}", s.name, t.name), t.state))
        .collect()
}

fn named(states: &[(&str, TaskState)]) -> Vec<(String, TaskState)> {
    states.iter().map(|(n, s)| (n.to_string(), *s)).collect()
}

const SHARED_STAGE: &str = "from bettertest import Stage, task


class Deploy(Stage):
    worker = \"WORKER\"
    image = \"debian\"

    @task()
    def push():
        pass

    @task()
    def smoke():
        pass

    @task()
    def check():
        pass
";

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_sandbox_runs_its_tasks_in_order_and_stops_at_the_first_failure() {
    let (url, calls) = sandbox_worker(vec![0, 1], StatusCode::NO_CONTENT).await;
    let (boss, run) = run_to_end(&url, SHARED_STAGE).await;
    assert_eq!(run.state, RunState::Fail);
    assert_eq!(
        states(&run),
        named(&[
            ("Deploy.push", Pass),
            ("Deploy.smoke", Fail),
            ("Deploy.check", Skipped)
        ])
    );
    assert_eq!(
        *lock(&calls),
        [
            "start c1",
            "push in container:c1",
            "smoke in container:c1",
            "stop c1"
        ]
    );
    // Starting the sandbox shows in the first task's log.
    let push = log_text(&boss, run.stages[0].tasks[0].id);
    assert_eq!(
        push[..2],
        ["starting container debian", "started container c1"]
    );
}

/// A sandbox that does not stop is a leak, not a pass: the last task that ran fails for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_sandbox_that_will_not_stop_fails_its_last_task() {
    let (url, calls) = sandbox_worker(vec![0, 0, 0], StatusCode::INTERNAL_SERVER_ERROR).await;
    let (boss, run) = run_to_end(&url, SHARED_STAGE).await;
    assert_eq!(run.state, RunState::Fail);
    assert_eq!(
        states(&run),
        named(&[
            ("Deploy.push", Pass),
            ("Deploy.smoke", Pass),
            ("Deploy.check", Fail)
        ])
    );
    assert_eq!(lock(&calls).last().map(String::as_str), Some("stop c1"));
    let check = log_text(&boss, run.stages[0].tasks[2].id);
    let said = format!("{url}/container/c1 said 500 Internal Server Error: docker rm failed");
    assert!(check.contains(&said), "{check:?}");
}

const SPANNING: &str = "from bettertest import Sandbox, Stage, task

DEBIAN = Sandbox(worker=\"WORKER\", image=\"debian\")


class Install(Stage):
    @task(sandbox=DEBIAN)
    def debian():
        pass


class Reboot(Stage):
    @task(sandbox=DEBIAN)
    def debian():
        pass


class Check(Stage):
    @task(sandbox=DEBIAN)
    def debian():
        pass
";

/// A pipeline's sandbox is started once, by the first stage that uses it, and stopped after the
/// last one.
#[tokio::test(flavor = "multi_thread")]
async fn a_pipeline_sandbox_lives_across_its_stages() {
    let (url, calls) = sandbox_worker(vec![0, 0, 0], StatusCode::NO_CONTENT).await;
    let (_, run) = run_to_end(&url, SPANNING).await;
    assert_eq!(run.state, RunState::Pass);
    let target = "container:c1";
    assert_eq!(
        *lock(&calls),
        [
            "start c1".to_string(),
            format!("debian in {target}"),
            format!("debian in {target}"),
            format!("debian in {target}"),
            "stop c1".to_string()
        ]
    );
}

/// When a stage fails, the stages after it are skipped, and the sandbox they would have used is
/// stopped all the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_run_still_stops_a_pipeline_sandbox() {
    let (url, calls) = sandbox_worker(vec![0, 1], StatusCode::NO_CONTENT).await;
    let (_, run) = run_to_end(&url, SPANNING).await;
    assert_eq!(run.state, RunState::Fail);
    assert_eq!(
        states(&run),
        named(&[
            ("Install.debian", Pass),
            ("Reboot.debian", Fail),
            ("Check.debian", Skipped)
        ])
    );
    let calls = lock(&calls).clone();
    assert_eq!(calls.first().map(String::as_str), Some("start c1"));
    assert_eq!(calls.last().map(String::as_str), Some("stop c1"));
    assert_eq!(calls.iter().filter(|c| c.starts_with("stop")).count(), 1);
}

/// git in `dir`, as a test commits: with a name of its own, touching nobody's config.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@bettertest.dev",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A forge in miniature: repo team/app served over git's dumb HTTP, a work tree to commit to it
/// from, and the commit statuses posted to its API.
struct Forge {
    root: PathBuf,
    work: PathBuf,
    url: String,
    statuses: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Forge {
    async fn new() -> Forge {
        let root = std::env::temp_dir().join(format!("forge-{}", Uuid::new_v4().simple()));
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        let statuses: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let (files, posted) = (root.clone(), statuses.clone());
        let serve = move |AxumPath(path): AxumPath<String>| async move {
            match fs::read(files.join(&path)) {
                Ok(bytes) => (StatusCode::OK, bytes),
                Err(_) => (StatusCode::NOT_FOUND, vec![]),
            }
        };
        let status = move |AxumPath(sha): AxumPath<String>, Json(body): Json<Value>| async move {
            lock(&posted).push((sha, body));
            StatusCode::CREATED
        };
        let app = Router::new()
            .route("/api/v1/repos/team/app/statuses/{sha}", post(status))
            .route("/{*path}", get(serve));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/team/app.git", listener.local_addr().unwrap());
        spawn(async move { axum::serve(listener, app).await.unwrap() });
        Forge {
            root,
            work,
            url,
            statuses,
        }
    }

    /// Commits `files` to main and publishes it. The commit's sha.
    fn commit(&self, files: &[(&str, &str)]) -> String {
        for (path, text) in files {
            let path = self.work.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        git(&self.work, &["add", "."]);
        git(&self.work, &["commit", "--quiet", "-m", "change"]);
        let bare = self.root.join("team/app.git");
        fs::remove_dir_all(&bare).ok();
        let (work, bare_path) = (self.work.to_str().unwrap(), bare.to_str().unwrap());
        git(&self.root, &["clone", "--quiet", "--bare", work, bare_path]);
        git(&bare, &["update-server-info"]);
        git(&self.work, &["rev-parse", "HEAD"])
    }

    /// The statuses posted so far, as (sha, context, state, description).
    fn statuses(&self) -> BTreeSet<(String, String, String, String)> {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let posted = lock(&self.statuses);
        (posted.iter())
            .map(|(sha, b)| {
                let fields = (
                    text(&b["context"]),
                    text(&b["state"]),
                    text(&b["description"]),
                );
                (sha.clone(), fields.0, fields.1, fields.2)
            })
            .collect()
    }
}

impl Drop for Forge {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A push starts every pipeline the pushed commit has, at that commit, and tells the forge how each
/// does. A newer commit on the branch replaces the run of the older one. Tags, deleted branches and
/// other repos start nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_runs_its_commit_and_a_newer_one_replaces_it() {
    let forge = Forge::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let worker = worker_hanging_once(1, calls.clone()).await;
    let app = format!(
        "from bettertest import Stage, task\n\n\nclass Test(Stage):\n    worker = \"{worker}\"\n\n    \
         @task()\n    def unit():\n        pass\n"
    );
    let first = forge.commit(&[
        (".bettertest/app.py", &app),
        (".bettertest/broken.py", "this is not python("),
        ("README", "one"),
    ]);
    let repo = Repo::new(&forge.url).unwrap();
    let mirror = workdir::dir("repos")
        .unwrap()
        .join(crate::worker::cache_key(&repo.public));
    let mut owned = Arc::into_inner(boss(workers(&[(&worker, &["x86"], 0, 4)]))).unwrap();
    owned.repos = vec![repo];
    let boss = Arc::new(owned);
    boss.fetch(0).await;
    assert_eq!(boss.pipelines(), ["app", "broken"]);

    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "ci.example:7357".parse().unwrap());
    let push = |git_ref: &str, after: &str, repo: &str| {
        let push = Push {
            git_ref: git_ref.into(),
            after: after.into(),
            repository: PushedRepo {
                full_name: repo.into(),
            },
        };
        push_hook(State(boss.clone()), headers.clone(), Json(push))
    };
    let nothing = |pushed: Result<Json<Vec<Run>>, Failure>| pushed.unwrap().0.is_empty();
    assert!(nothing(push("refs/tags/v1", &first, "team/app").await));
    assert!(nothing(
        push("refs/heads/main", &"0".repeat(40), "team/app").await
    ));
    let other = push("refs/heads/main", &first, "team/other").await;
    assert_eq!(other.map(|_| ()).unwrap_err(), not_found("repo team/other"));

    let mut events = boss.events.subscribe();
    let Json(runs) = push("refs/heads/main", &first, "Team/App").await.unwrap();
    let [replaced] = &runs[..] else {
        panic!("{runs:?}")
    };
    let commit = |sha: &str| {
        Some(Commit {
            sha: sha.into(),
            branch: "main".into(),
        })
    };
    assert_eq!(
        (replaced.pipeline.as_str(), &replaced.commit),
        ("app", &commit(&first))
    );
    let running = async {
        loop {
            if let Ok(Event::Log { line, .. }) = events.recv().await
                && line.line == "first try"
            {
                return;
            }
        }
    };
    timeout(Duration::from_secs(20), running).await.unwrap();

    let second = forge.commit(&[("README", "two")]);
    let Json(runs) = push("refs/heads/main", &second, "team/app").await.unwrap();
    let [newer] = &runs[..] else {
        panic!("{runs:?}")
    };
    assert_eq!(newer.commit, commit(&second));
    let mut ended = BTreeMap::new();
    let both = async {
        while ended.len() < 2 {
            if let Ok(Event::RunFinished { run, state, .. }) = events.recv().await {
                ended.insert(run, state);
            }
        }
    };
    timeout(Duration::from_secs(20), both).await.unwrap();
    assert_eq!(ended[&replaced.id], RunState::Fail);
    assert_eq!(ended[&newer.id], RunState::Pass);
    let label = format!("main @ {}", &second[..7]);
    let cancelled = format!("cancelled: {label} replaced it");
    let unit = replaced.stages[0].tasks[0].id;
    let log = log_text(&boss, unit);
    assert!(log.contains(&cancelled), "{log:?}");

    // The source a task of the newer run unpacks is that commit's.
    let source = get_source(State(boss.clone()), AxumPath(newer.id.to_string()))
        .await
        .unwrap();
    let tar = axum::body::to_bytes(source.into_body(), usize::MAX)
        .await
        .unwrap();
    let names = tar_names(&tar);
    assert_eq!(
        names,
        [
            ".bettertest/",
            ".bettertest/app.py",
            ".bettertest/broken.py",
            "README"
        ]
    );
    // git archive names the commit in its first header.
    assert!(tar.windows(40).any(|w| w == second.as_bytes()));

    // What the forge heard, once the last report is in.
    let expected = BTreeSet::from(
        [
            (&first, "broken", "failure", "the pipedef is broken"),
            (&first, "app", "pending", "running"),
            (&first, "app", "warning", cancelled.as_str()),
            (&second, "broken", "failure", "the pipedef is broken"),
            (&second, "app", "pending", "running"),
            (&second, "app", "success", "passed"),
        ]
        .map(|(sha, context, state, said)| {
            let context = format!("bettertest/{context}");
            (sha.clone(), context, state.to_string(), said.to_string())
        }),
    );
    let heard = async {
        while forge.statuses().len() < expected.len() {
            sleep(Duration::from_millis(10)).await;
        }
    };
    timeout(Duration::from_secs(10), heard).await.unwrap();
    assert_eq!(forge.statuses(), expected);
    let links: BTreeSet<String> = lock(&forge.statuses)
        .iter()
        .map(|(_, body)| body["target_url"].to_string())
        .collect();
    assert_eq!(
        links,
        BTreeSet::from([
            "\"http://ci.example:7357/pipeline/app\"".to_string(),
            "\"http://ci.example:7357/pipeline/broken\"".to_string()
        ])
    );
    fs::remove_dir_all(mirror).unwrap();
}

/// The names of the files in a tar archive, in order.
fn tar_names(tar: &[u8]) -> Vec<String> {
    let mut names = vec![];
    let mut at = 0;
    while let Some(header) = tar.get(at..at + 512) {
        let name: Vec<u8> = header[..100]
            .iter()
            .copied()
            .take_while(|&b| b != 0)
            .collect();
        if name.is_empty() {
            break;
        }
        let size = std::str::from_utf8(&header[124..135]).unwrap();
        let size = usize::from_str_radix(size.trim_matches(['\0', ' ']), 8).unwrap_or(0);
        let name = String::from_utf8(name).unwrap();
        // git archive starts with a pax header holding the commit id.
        if name != "pax_global_header" {
            names.push(name);
        }
        at += 512 + size.div_ceil(512) * 512;
    }
    names
}

/// A stand-in worker whose task publishes `published` (`<id>/<name>`, as its artifact event says)
/// and exits 0. It serves the file at /output/<id>/app.deb only.
async fn worker_publishing(published: String) -> String {
    let stream = format!("event: artifact\ndata: {published}\n\nevent: exit\ndata: 0\n\n");
    let start_task = move || async move { ([(header::CONTENT_TYPE, "text/event-stream")], stream) };
    let output = |AxumPath((_, name)): AxumPath<(String, String)>| async move {
        match name.as_str() {
            "app.deb" => (StatusCode::OK, "deb!"),
            _ => (StatusCode::NOT_FOUND, "no such file"),
        }
    };
    let app = Router::new()
        .route("/start-task", post(start_task))
        .route("/output/{id}/{name}", get(output));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

const ONE_TASK: &str = "from bettertest import Stage, task


class Build(Stage):
    worker = \"WORKER\"

    @task()
    def packages():
        pass
";

/// What a task publishes, the boss downloads from its worker before hanging up, and keeps. A name
/// that could reach outside the task's directory, or a file the worker does not have, fails the task.
#[tokio::test(flavor = "multi_thread")]
async fn the_boss_keeps_what_a_task_publishes() {
    let id = Uuid::new_v4();
    let url = worker_publishing(format!("{id}/app.deb")).await;
    let (boss, run) = run_to_end(&url, ONE_TASK).await;
    assert_eq!(run.state, RunState::Pass);
    let task = run.stages[0].tasks[0].id;
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 4,
    };
    assert_eq!(boss.db().artifacts(task).unwrap(), [artifact]);
    let dir = task_artifacts_dir(task).unwrap();
    assert_eq!(fs::read_to_string(dir.join("app.deb")).unwrap(), "deb!");
    // Nothing half-written is left beside it.
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    assert_eq!(
        log_text(&boss, task),
        ["kept artifact app.deb (4 bytes)", "exit code 0"]
    );
    fs::remove_dir_all(dir).unwrap();

    for (published, said) in [
        (
            format!("{id}/../bettertest.db"),
            format!("the worker published a bad artifact \"{id}/../bettertest.db\""),
        ),
        (
            "not-a-uuid/app.deb".to_string(),
            "the worker published a bad artifact \"not-a-uuid/app.deb\"".to_string(),
        ),
    ] {
        let url = worker_publishing(published).await;
        let (boss, run) = run_to_end(&url, ONE_TASK).await;
        assert_eq!(run.state, RunState::Fail);
        let log = log_text(&boss, run.stages[0].tasks[0].id);
        assert_eq!(log.last(), Some(&said), "{log:?}");
    }

    let url = worker_publishing(format!("{id}/app.rpm")).await;
    let (boss, run) = run_to_end(&url, ONE_TASK).await;
    assert_eq!(run.state, RunState::Fail);
    let task = run.stages[0].tasks[0].id;
    let log = log_text(&boss, task);
    let said = format!("{url}/output/{id}/app.rpm said 404 Not Found: no such file");
    assert_eq!(log.last(), Some(&said), "{log:?}");
    assert!(boss.db().artifacts(task).unwrap().is_empty());
}
