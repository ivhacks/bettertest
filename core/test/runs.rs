// Tests of boss/runs.rs: stages whose tasks share a sandbox, and a pipeline's sandboxes that live
// across stages, run against a stand-in worker. A child of the boss's tests (see the #[path] in
// test/boss.rs), so it reaches what the boss keeps private.

use super::*;
use axum::extract::Multipart;
use serde_json::json;

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
