// Tests of boss/mod.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use std::collections::BTreeSet;

fn task(name: &str) -> Task {
    Task {
        name: name.into(),
        worker: None,
        labels: vec!["x86".into()],
        image: None,
        vm: None,
        arch: None,
        timeout: None,
        sandbox: None,
    }
}

// Build (compile), Test (unit, e2e), Deploy (shared sandbox: push, smoke).
fn pipeline() -> Pipeline {
    let stage = |name: &str, tasks: &[&str]| Stage {
        name: name.into(),
        sandbox: None,
        tasks: tasks.iter().map(|t| task(t)).collect(),
    };
    let mut deploy = stage("Deploy", &["push", "smoke"]);
    deploy.sandbox = Some(Sandbox {
        worker: None,
        labels: vec!["x86".into()],
        image: Some("debian".into()),
        vm: None,
        arch: None,
        parallel: false,
    });
    Pipeline {
        stages: vec![
            stage("Build", &["compile"]),
            stage("Test", &["unit", "e2e"]),
            deploy,
        ],
        sandboxes: Default::default(),
    }
}

fn run(states: &[(&str, &[(&str, TaskState)])]) -> Run {
    Run {
        id: Uuid::from_u128(0xabcdef),
        pipeline: "app".into(),
        state: RunState::Fail,
        stages: states
            .iter()
            .map(|(stage, tasks)| StageRun {
                name: stage.to_string(),
                tasks: tasks
                    .iter()
                    .map(|(name, state)| TaskRun {
                        id: Uuid::new_v4(),
                        name: name.to_string(),
                        state: *state,
                        worker: None,
                        started: None,
                        ended: None,
                        carried_from: None,
                    })
                    .collect(),
            })
            .collect(),
        commit: None,
    }
}

fn names(carried: &BTreeMap<(String, String), TaskRun>) -> Vec<String> {
    carried.keys().map(|(s, t)| format!("{s}.{t}")).collect()
}

use TaskState::{Fail, Pass, Skipped};

#[test]
fn rerunning_a_task_carries_over_what_came_before_and_its_siblings() {
    let earlier = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass), ("e2e", Fail)]),
        ("Deploy", &[("push", Skipped), ("smoke", Skipped)]),
    ]);
    let carried = carry_over(&earlier, &pipeline(), Some("Test"), Some("e2e"));
    assert_eq!(names(&carried.unwrap()), ["Build.compile", "Test.unit"]);
    // The whole stage, then.
    let carried = carry_over(&earlier, &pipeline(), Some("Test"), None);
    assert_eq!(names(&carried.unwrap()), ["Build.compile"]);
    // And the whole run.
    let carried = carry_over(&earlier, &pipeline(), None, None);
    assert!(carried.unwrap().is_empty());
}

#[test]
fn a_failed_earlier_stage_cannot_be_carried_over() {
    let earlier = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass), ("e2e", Fail)]),
        ("Deploy", &[("push", Skipped), ("smoke", Skipped)]),
    ]);
    let refused = carry_over(&earlier, &pipeline(), Some("Deploy"), None);
    assert_eq!(
        refused.unwrap_err(),
        (
            StatusCode::CONFLICT,
            "Test.e2e failed in run 000000, so the stages after it have nothing to build on. \
             Rerun Test instead."
                .to_string()
        )
    );
}

#[test]
fn a_task_in_a_shared_sandbox_reruns_its_whole_stage() {
    let earlier = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass), ("e2e", Pass)]),
        ("Deploy", &[("push", Pass), ("smoke", Fail)]),
    ]);
    let carried = carry_over(&earlier, &pipeline(), Some("Deploy"), Some("smoke"));
    assert_eq!(
        names(&carried.unwrap()),
        ["Build.compile", "Test.e2e", "Test.unit"]
    );
}

#[test]
fn a_task_in_a_parallel_shared_sandbox_reruns_alone() {
    let earlier = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass), ("e2e", Pass)]),
        ("Deploy", &[("push", Pass), ("smoke", Fail)]),
    ]);
    let mut pipeline = pipeline();
    if let Some(sandbox) = &mut pipeline.stages[2].sandbox {
        sandbox.parallel = true;
    }
    let carried = carry_over(&earlier, &pipeline, Some("Deploy"), Some("smoke"));
    assert_eq!(
        names(&carried.unwrap()),
        ["Build.compile", "Deploy.push", "Test.e2e", "Test.unit"]
    );
}

// Build (compile), Install (debian, fedora), Reboot (debian, fedora), Check (web); the debian
// and fedora tasks run in the pipeline's sandboxes DEBIAN and FEDORA.
fn spanning_pipeline() -> Pipeline {
    let in_sandbox = |name: &str, sandbox: &str| Task {
        sandbox: Some(sandbox.into()),
        labels: vec![],
        ..task(name)
    };
    let stage = |name: &str, tasks: Vec<Task>| Stage {
        name: name.into(),
        sandbox: None,
        tasks,
    };
    let vm = |image: &str| Sandbox {
        worker: None,
        labels: vec!["x86".into()],
        image: None,
        vm: Some(image.into()),
        arch: None,
        parallel: false,
    };
    let both = || {
        vec![
            in_sandbox("debian", "DEBIAN"),
            in_sandbox("fedora", "FEDORA"),
        ]
    };
    Pipeline {
        stages: vec![
            stage("Build", vec![task("compile")]),
            stage("Install", both()),
            stage("Reboot", both()),
            stage("Check", vec![task("web")]),
        ],
        sandboxes: [
            ("DEBIAN".into(), vm("debian")),
            ("FEDORA".into(), vm("fedora")),
        ]
        .into(),
    }
}

#[test]
fn a_sandbox_spanning_stages_stops_after_the_last_one_using_it() {
    let pipeline = spanning_pipeline();
    let names = |index| {
        last_used_in(&pipeline, index)
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(names(0), Vec::<String>::new());
    assert_eq!(names(1), Vec::<String>::new());
    assert_eq!(names(2), ["DEBIAN", "FEDORA"]);
    assert_eq!(names(3), Vec::<String>::new());
}

#[test]
fn rerunning_a_stage_that_uses_a_spanning_sandbox_reruns_from_its_first_stage() {
    let pipeline = spanning_pipeline();
    // Reboot needs DEBIAN and FEDORA as Install left them: Install reruns too.
    assert_eq!(rerun_start(&pipeline, 2, None), (1, None));
    assert_eq!(rerun_start(&pipeline, 2, Some("debian")), (1, None));
    // Install is where they start, but a task in a sandbox reruns its whole stage.
    assert_eq!(rerun_start(&pipeline, 1, Some("debian")), (1, None));
    // Check and Build use no sandbox: as before.
    assert_eq!(rerun_start(&pipeline, 3, Some("web")), (3, Some("web")));
    assert_eq!(rerun_start(&pipeline, 0, None), (0, None));
    let earlier = run(&[
        ("Build", &[("compile", Pass)]),
        ("Install", &[("debian", Pass), ("fedora", Pass)]),
        ("Reboot", &[("debian", Fail), ("fedora", Pass)]),
        ("Check", &[("web", Skipped)]),
    ]);
    let carried = carry_over(&earlier, &pipeline, Some("Reboot"), Some("debian"));
    assert_eq!(names(&carried.unwrap()), ["Build.compile"]);
}

#[test]
fn tasks_the_pipedef_gained_since_run() {
    // The earlier run predates Test.e2e.
    let earlier = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass)]),
        ("Deploy", &[("push", Pass), ("smoke", Fail)]),
    ]);
    let carried = carry_over(&earlier, &pipeline(), Some("Deploy"), None);
    assert_eq!(names(&carried.unwrap()), ["Build.compile", "Test.unit"]);
}

// Each worker: url, labels, tasks running, tasks it takes.
fn workers(up: &[(&str, &[&str], usize, u32)]) -> BTreeMap<String, Watched> {
    up.iter()
        .map(|(url, labels, busy, capacity)| {
            let task = WorkerTask {
                id: Uuid::nil(),
                pipeline: "app".into(),
                run: Uuid::nil(),
                stage: "Build".into(),
                task: "compile".into(),
            };
            let worker = Worker {
                url: url.to_string(),
                info: Some(WorkerInfo {
                    hostname: "host".into(),
                    arch: Arch::X86_64,
                    version: "0.1.0".into(),
                    labels: labels.iter().map(|l| l.to_string()).collect(),
                    capacity: *capacity,
                }),
                since: 0,
                problem: None,
                tasks: vec![task; *busy],
            };
            let watched = Watched {
                worker,
                watcher: None,
            };
            (url.to_string(), watched)
        })
        .collect()
}

fn picked(up: &BTreeMap<String, Watched>, labels: &[&str]) -> String {
    picked_on(up, None, labels)
}

fn picked_on(up: &BTreeMap<String, Watched>, named: Option<&str>, labels: &[&str]) -> String {
    let labels: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
    match pick_worker(up, named, &labels) {
        Ok(Pick::Worker(url)) => url,
        Ok(Pick::Busy(why)) => format!("busy: {why}"),
        Err(why) => format!("none: {why}"),
    }
}

#[test]
fn the_only_matching_worker_is_picked() {
    // The arm machine is idle and the x86 one is busy. The task asked for x86, so it lands there.
    let up = workers(&[
        ("http://arm", &["arm"], 0, 4),
        ("http://x86", &["x86"], 3, 4),
    ]);
    assert_eq!(picked(&up, &["x86"]), "http://x86");
    // A worker with more labels than the task asked for still covers it. One that lacks one does not.
    let up = workers(&[
        ("http://a", &["x86"], 0, 1),
        ("http://b", &["x86", "gpu"], 0, 1),
    ]);
    assert_eq!(picked(&up, &["x86", "gpu"]), "http://b");
}

#[test]
fn any_of_several_matching_workers_can_be_picked() {
    // b is busier than a. Both have the labels and a free slot, so either may run the task.
    // c has the wrong labels and must not.
    let up = workers(&[
        ("http://a", &["x86"], 0, 2),
        ("http://b", &["x86"], 1, 2),
        ("http://c", &["arm"], 0, 2),
    ]);
    let mut seen = BTreeSet::new();
    for _ in 0..200 {
        let got = picked(&up, &["x86"]);
        assert_ne!(got, "http://c", "scheduled on a worker without the labels");
        seen.insert(got);
    }
    assert_eq!(
        seen,
        ["http://a".to_string(), "http://b".to_string()].into()
    );
}

#[test]
fn no_match_is_not_scheduled_on_another_worker() {
    let up = workers(&[
        ("http://arm", &["arm"], 0, 1),
        ("http://gpu", &["x86", "gpu"], 0, 1),
    ]);
    let got = picked(&up, &["arm", "gpu"]);
    assert!(
        got.starts_with("none: no connected worker has labels arm, gpu"),
        "{got}"
    );
    assert!(
        !got.starts_with("http://"),
        "a miss must not invent a worker: {got}"
    );
}

#[test]
fn a_full_worker_gets_no_more_tasks() {
    // a takes one task and runs one; b takes two and runs one.
    let up = workers(&[("http://a", &["x86"], 1, 1), ("http://b", &["x86"], 1, 2)]);
    assert_eq!(picked(&up, &["x86"]), "http://b");
    let up = workers(&[("http://a", &["x86"], 1, 1), ("http://b", &["x86"], 2, 2)]);
    assert_eq!(
        picked(&up, &["x86"]),
        "busy: every worker with labels x86 is running as many tasks as it takes"
    );
    // The only free worker has the wrong labels. The task waits; it does not run there.
    let up = workers(&[("http://a", &["x86"], 1, 1), ("http://b", &["arm"], 0, 1)]);
    assert_eq!(
        picked(&up, &["x86"]),
        "busy: every worker with labels x86 is running as many tasks as it takes"
    );
    // A worker still connecting may be the one: how many tasks it takes is not known yet.
    // gpu is a label nobody connected has, so the boss waits rather than guessing.
    let mut up = up;
    let connecting = Worker {
        url: "http://d".into(),
        info: None,
        since: 0,
        problem: None,
        tasks: vec![],
    };
    let watched = Watched {
        worker: connecting,
        watcher: None,
    };
    up.insert("http://d".into(), watched);
    assert_eq!(
        picked(&up, &["gpu"]),
        "busy: no connected worker has labels gpu yet, and some are still connecting"
    );
    // Once that attempt has failed, a label nobody has is an error at once.
    if let Some(d) = up.get_mut("http://d") {
        d.worker.problem = Some("http://d did not answer".into());
    }
    let got = picked(&up, &["gpu"]);
    assert!(
        got.starts_with("none: no connected worker has labels gpu"),
        "{got}"
    );
    assert!(!got.starts_with("http://"), "{got}");
}

#[test]
fn a_sandbox_stays_on_the_worker_that_already_has_it() {
    let up = workers(&[("http://a", &["x86"], 0, 1), ("http://b", &["x86"], 0, 1)]);
    assert_eq!(picked_on(&up, Some("http://b"), &[]), "http://b");
    let up = workers(&[("http://a", &["x86"], 1, 1)]);
    assert_eq!(
        picked_on(&up, Some("http://a"), &[]),
        "busy: http://a is running as many tasks as it takes"
    );
    // Still connecting: wait, rather than start the next task of the sandbox elsewhere.
    let mut up = workers(&[]);
    up.insert(
        "http://d".into(),
        Watched {
            worker: Worker {
                url: "http://d".into(),
                info: None,
                since: 0,
                problem: None,
                tasks: vec![],
            },
            watcher: None,
        },
    );
    assert_eq!(
        picked_on(&up, Some("http://d"), &[]),
        "busy: http://d is connecting"
    );
}

#[test]
fn a_named_worker_url_is_the_one_that_runs() {
    // Another x86 worker is free. The pipedef named this url, so the task runs there.
    let up = workers(&[
        ("http://a", &["x86"], 0, 2),
        ("http://pinned", &["arm"], 0, 1),
    ]);
    assert_eq!(picked_on(&up, Some("http://pinned"), &[]), "http://pinned");
    let up = workers(&[("http://pinned", &["x86"], 1, 1)]);
    assert_eq!(
        picked_on(&up, Some("http://pinned"), &[]),
        "busy: http://pinned is running as many tasks as it takes"
    );
}

#[test]
fn labels_one_typo_away_are_named() {
    assert_eq!(edit_distance("gpu", "gpu"), 0);
    assert_eq!(edit_distance("gpu", "gpuu"), 1);
    assert_eq!(edit_distance("pcba-rig", "pcb-rigg"), 2);
    assert_eq!(edit_distance("", "arm"), 3);
    assert!(similar_label("GPU", "gpu"));
    assert!(similar_label("gpuu", "gpu"));
    assert!(similar_label("pcba-rig", "pcba_rigs"));
    assert!(!similar_label("arm", "x86"));
    assert!(!similar_label("gpu", "cpu1"));
    let up = workers(&[
        ("http://a", &["x86", "gpu"], 0, 1),
        ("http://b", &["arm"], 0, 1),
    ]);
    assert_eq!(
        picked(&up, &["x86", "gpuu"]),
        "none: no connected worker has labels x86, gpuu. Worker http://a has the similar label \
         gpu. Connected workers: http://a (x86, gpu); http://b (arm). The workers page shows \
         them all."
    );
}

#[test]
fn unknown_stages_and_tasks() {
    let earlier = run(&[("Build", &[("compile", Pass)])]);
    let missing = carry_over(&earlier, &pipeline(), Some("Lint"), None);
    assert_eq!(missing.unwrap_err().1, "no stage Lint in app");
    let missing = carry_over(&earlier, &pipeline(), Some("Build"), Some("unit"));
    assert_eq!(missing.unwrap_err().1, "no task Build.unit in app");
}

/// A boss with an empty db in memory, which knows the workers `up`.
fn boss(up: BTreeMap<String, Watched>) -> Shared {
    let db = db::prepare(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
    Arc::new(Boss {
        pipedefs: Mutex::new(BTreeMap::new()),
        repos: vec![],
        going: Mutex::new(HashMap::new()),
        db: Mutex::new(db),
        workers: Mutex::new(up),
        task_tokens: Mutex::new(HashMap::new()),
        ceremonies: Mutex::new(HashMap::new()),
        events: broadcast::channel(4096).0,
        freed: Notify::new(),
        http: client::http().unwrap(),
        tls: None,
        port: LISTEN_PORT,
    })
}

/// A worker that runs every task it is sent in a moment.
async fn quick_worker() -> String {
    let start_task = || async {
        sleep(Duration::from_millis(5)).await;
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "event: exit\ndata: 0\n\n",
        )
    };
    let app = Router::new().route("/start-task", post(start_task));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// Runs a `parallel` stage of tasks t0 to t39 on a worker that takes two at once, rerunning task
/// `rerun` of `earlier` if given: whether it passed, and the tasks it started, in order.
async fn parallel_stage(rerun: Option<(&Run, &str)>) -> (bool, Vec<String>) {
    let url = quick_worker().await;
    let boss = boss(workers(&[(&url, &["x86"], 0, 2)]));
    let mut events = boss.events.subscribe();
    let sandbox = Sandbox {
        worker: Some(url),
        labels: vec![],
        image: None,
        vm: None,
        arch: None,
        parallel: true,
    };
    let stage = Stage {
        name: "Test".into(),
        sandbox: Some(sandbox.clone()),
        tasks: (0..40).map(|i| task(&format!("t{i}"))).collect(),
    };
    let pipeline = Pipeline {
        stages: vec![stage.clone()],
        sandboxes: Default::default(),
    };
    let carried = match rerun {
        Some((earlier, name)) => carry_over(earlier, &pipeline, Some("Test"), Some(name)).unwrap(),
        None => BTreeMap::new(),
    };
    let run = boss
        .db()
        .insert_run("app", &pipeline, None, &carried)
        .unwrap();
    let r = Running {
        boss: boss.clone(),
        run: Arc::new(run.clone()),
        source: "".into(),
        cancelled: watch::channel(None).1,
        spanning: Spanning::default(),
    };
    let passed = run_shared_stage(&r, &stage, &sandbox, &run.stages[0]).await;
    let mut started = vec![];
    while let Ok(event) = events.try_recv() {
        if let Event::TaskStarted { task, .. } = event
            && let Some((_, name)) = run.locate(task)
        {
            started.push(name.to_string());
        }
    }
    (passed, started)
}

/// An earlier run of `parallel_stage` in which t7 failed.
fn t7_failed() -> Run {
    let names: Vec<String> = (0..40).map(|i| format!("t{i}")).collect();
    let tasks: Vec<(&str, TaskState)> = (names.iter())
        .map(|n| (n.as_str(), if n == "t7" { Fail } else { Pass }))
        .collect();
    run(&[("Test", &tasks)])
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parallel_stage_starts_its_tasks_in_the_order_the_pipedef_defines_them() {
    let names: Vec<String> = (0..40).map(|i| format!("t{i}")).collect();
    assert_eq!(parallel_stage(None).await, (true, names));
}

#[tokio::test(flavor = "multi_thread")]
async fn rerunning_a_task_of_a_parallel_stage_runs_only_that_task() {
    let earlier = t7_failed();
    assert_eq!(
        parallel_stage(Some((&earlier, "t7"))).await,
        (true, vec!["t7".to_string()])
    );
    // A sibling that failed before still fails the stage.
    assert_eq!(
        parallel_stage(Some((&earlier, "t8"))).await,
        (false, vec!["t8".to_string()])
    );
}
