// Tests of lib.rs, a child module of it (see the #[path] at its end), so they reach what it keeps
// private.

use super::*;

fn run(n: u128) -> Run {
    Run {
        id: Uuid::from_u128(n),
        pipeline: "app".into(),
        state: RunState::Running,
        stages: vec![StageRun {
            name: "Test".into(),
            tasks: vec![TaskRun {
                id: Uuid::from_u128(n + 100),
                name: "unit".into(),
                state: TaskState::Pending,
                worker: None,
                started: None,
                ended: None,
                carried_from: None,
                tests: None,
            }],
        }],
        commit: None,
    }
}

#[test]
fn an_event_replaces_only_the_run_it_changes() {
    let mut snapshot = PipelineSnapshot {
        name: "app".into(),
        pipelines: vec!["app".into()],
        pipeline: Err("not needed".into()),
        runs: vec![Arc::new(run(2)), Arc::new(run(1))],
    };
    let before = snapshot.runs.clone();
    snapshot.apply(&Event::TaskStarted {
        pipeline: "app".into(),
        run: Uuid::from_u128(1),
        task: Uuid::from_u128(101),
        worker: "http://w:7358".into(),
        at: 5,
    });
    let [newer, older] = [&snapshot.runs[0], &snapshot.runs[1]];
    assert!(Arc::ptr_eq(newer, &before[0]));
    assert!(!Arc::ptr_eq(older, &before[1]));
    assert_eq!(older.stages[0].tasks[0].state, TaskState::Running);
    assert_eq!(older.stages[0].tasks[0].started, Some(5));
    // An event about no run here changes nothing, not even a pointer.
    let before = snapshot.runs.clone();
    snapshot.apply(&Event::RunFinished {
        pipeline: "app".into(),
        run: Uuid::from_u128(9),
        state: RunState::Pass,
    });
    assert!(
        snapshot
            .runs
            .iter()
            .zip(&before)
            .all(|(a, b)| Arc::ptr_eq(a, b))
    );
}

#[test]
fn test_counts_land_on_their_task() {
    let tests = Tests {
        total: 1234,
        failed: 2,
    };
    let event = Event::TaskTests {
        pipeline: "app".into(),
        run: Uuid::from_u128(1),
        task: Uuid::from_u128(101),
        tests,
    };
    assert_eq!(event.run(), Some(("app", Uuid::from_u128(1))));
    let mut snapshot = PipelineSnapshot {
        name: "app".into(),
        pipelines: vec!["app".into()],
        pipeline: Err("not needed".into()),
        runs: vec![Arc::new(run(2)), Arc::new(run(1))],
    };
    snapshot.apply(&event);
    assert_eq!(snapshot.runs[1].stages[0].tasks[0].tests, Some(tests));
    assert_eq!(snapshot.runs[0].stages[0].tasks[0].tests, None);
    let mut snapshot = TaskSnapshot {
        now: 0,
        run: run(1),
        carried_from: None,
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Test".into(),
        task: run(1).stages[0].tasks[0].clone(),
        lines: vec![],
        artifacts: vec![],
    };
    snapshot.apply(&event);
    assert_eq!(snapshot.task.tests, Some(tests));
    assert_eq!(snapshot.run.stages[0].tasks[0].tests, Some(tests));
}

#[test]
fn workers_stay_sorted_by_url_one_each() {
    let worker = |url: &str, since| Worker {
        url: url.into(),
        info: None,
        since,
        problem: None,
        tasks: vec![],
    };
    let mut snapshot = WorkersSnapshot {
        now: 0,
        workers: vec![worker("http://b", 0), worker("http://d", 0)],
    };
    let urls = |s: &WorkersSnapshot| -> Vec<(String, u64)> {
        s.workers.iter().map(|w| (w.url.clone(), w.since)).collect()
    };
    for (url, since) in [
        ("http://c", 1),
        ("http://a", 2),
        ("http://e", 3),
        ("http://d", 4),
    ] {
        snapshot.apply(&Event::Worker(worker(url, since)));
    }
    let want = [
        ("http://a", 2),
        ("http://b", 0),
        ("http://c", 1),
        ("http://d", 4),
        ("http://e", 3),
    ];
    assert_eq!(urls(&snapshot), want.map(|(u, s)| (u.to_string(), s)));
    snapshot.apply(&Event::WorkerForgotten {
        url: "http://c".into(),
    });
    assert_eq!(snapshot.workers.len(), 4);
    assert!(snapshot.workers.iter().all(|w| w.url != "http://c"));
}

/// A task a restart puts back to pending (to run it again) keeps nothing of its last try: the db
/// forgets its worker, times, tests, carried-over result and artifacts, so a page that folds the
/// event has to forget them too, or it shows what a fresh snapshot no longer has.
#[test]
fn a_task_put_back_to_pending_keeps_nothing_of_its_last_try() {
    let mut ran = run(1);
    let earlier = Uuid::from_u128(7);
    ran.stages[0].tasks[0] = TaskRun {
        state: TaskState::Skipped,
        worker: Some("http://w:7358".into()),
        started: Some(5),
        ended: Some(9),
        carried_from: Some(Uuid::from_u128(77)),
        tests: Some(Tests {
            total: 3,
            failed: 0,
        }),
        ..run(1).stages[0].tasks[0].clone()
    };
    let rewound = Event::TaskState {
        pipeline: "app".into(),
        run: Uuid::from_u128(1),
        task: Uuid::from_u128(101),
        state: TaskState::Pending,
        at: 12,
    };
    let mut snapshot = PipelineSnapshot {
        name: "app".into(),
        pipelines: vec!["app".into()],
        pipeline: Err("not needed".into()),
        runs: vec![Arc::new(ran.clone())],
    };
    snapshot.apply(&rewound);
    assert_eq!(*snapshot.runs[0], run(1));

    let mut task = TaskSnapshot {
        now: 0,
        run: ran.clone(),
        carried_from: Some(earlier),
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Test".into(),
        task: ran.stages[0].tasks[0].clone(),
        lines: vec![LogLine {
            stream: LogStream::Stdout,
            line: "the earlier run's log".into(),
        }],
        artifacts: vec![Artifact {
            name: "app.deb".into(),
            size: 3,
        }],
    };
    task.apply(&rewound);
    let again = LogLine {
        stream: LogStream::Bettertest,
        line: "the boss restarted: running this again".into(),
    };
    task.apply(&Event::Log {
        task: Uuid::from_u128(101),
        line: again.clone(),
    });
    let fresh = TaskSnapshot {
        now: 0,
        run: run(1),
        carried_from: None,
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Test".into(),
        task: run(1).stages[0].tasks[0].clone(),
        lines: vec![again],
        artifacts: vec![],
    };
    assert_eq!(task, fresh);
}

/// A pipeline page holds the newest RECENT_RUNS runs of its own pipeline, however many start.
#[test]
fn a_page_keeps_only_its_pipelines_newest_runs() {
    let mut snapshot = PipelineSnapshot {
        name: "app".into(),
        pipelines: vec!["app".into(), "web".into()],
        pipeline: Err("not needed".into()),
        runs: vec![],
    };
    for n in 0..RECENT_RUNS as u128 + 5 {
        snapshot.apply(&Event::RunStarted(run(n)));
    }
    let mut web = run(1000);
    web.pipeline = "web".into();
    snapshot.apply(&Event::RunStarted(web));
    let ids: Vec<u128> = snapshot.runs.iter().map(|r| r.id.as_u128()).collect();
    let newest: Vec<u128> = (5..RECENT_RUNS as u128 + 5).rev().collect();
    assert_eq!(ids, newest);
    // A run that fell off the page changes nothing on it.
    let before = snapshot.runs.clone();
    snapshot.apply(&Event::RunFinished {
        pipeline: "app".into(),
        run: Uuid::from_u128(0),
        state: RunState::Pass,
    });
    assert!(
        snapshot
            .runs
            .iter()
            .zip(&before)
            .all(|(a, b)| Arc::ptr_eq(a, b))
    );
}

/// A task page takes the log lines and artifacts of its own task, and of no other.
#[test]
fn a_task_page_takes_only_its_own_tasks_lines_and_artifacts() {
    let mut snapshot = TaskSnapshot {
        now: 0,
        run: run(1),
        carried_from: None,
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Test".into(),
        task: run(1).stages[0].tasks[0].clone(),
        lines: vec![],
        artifacts: vec![],
    };
    let line = |text: &str| LogLine {
        stream: LogStream::Stderr,
        line: text.into(),
    };
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 3,
    };
    for task in [Uuid::from_u128(101), Uuid::from_u128(999)] {
        let said = format!("from {}", task.as_u128());
        snapshot.apply(&Event::Log {
            task,
            line: line(&said),
        });
        let artifact = artifact.clone();
        snapshot.apply(&Event::Artifact { task, artifact });
    }
    assert_eq!(snapshot.lines, [line("from 101")]);
    assert_eq!(snapshot.artifacts, [artifact]);
    snapshot.apply(&Event::RunFinished {
        pipeline: "app".into(),
        run: Uuid::from_u128(1),
        state: RunState::Fail,
    });
    assert_eq!(snapshot.run.state, RunState::Fail);
}

/// The workers a pipedef names by url, each once, sandboxes first: the boss enrolls each.
#[test]
fn a_pipedef_names_each_worker_once() {
    let task = |name: &str, worker: Option<&str>| Task {
        name: name.into(),
        worker: worker.map(Into::into),
        labels: vec![],
        image: None,
        vm: None,
        arch: None,
        timeout: None,
        sandbox: None,
    };
    let sandbox = |worker: Option<&str>| Sandbox {
        worker: worker.map(Into::into),
        labels: vec!["x86".into()],
        image: Some("debian".into()),
        vm: None,
        arch: Some(Arch::Aarch64),
        parallel: false,
    };
    let pipeline = Pipeline {
        stages: vec![
            Stage {
                name: "Build".into(),
                sandbox: None,
                tasks: vec![
                    task("a", Some("http://b:7358")),
                    task("b", None),
                    task("c", Some("http://a:7358")),
                ],
            },
            Stage {
                name: "Deploy".into(),
                sandbox: Some(sandbox(Some("http://a:7358"))),
                tasks: vec![task("d", Some("http://b:7358"))],
            },
        ],
        sandboxes: BTreeMap::from([
            ("ARM".to_string(), sandbox(Some("http://c:7358"))),
            ("ANY".to_string(), sandbox(None)),
        ]),
    };
    assert_eq!(
        pipeline.workers(),
        ["http://a:7358", "http://c:7358", "http://b:7358"]
    );
    assert_eq!(Arch::Aarch64.label(), "arm");
    assert_eq!(Arch::X86_64.label(), "x86");
}
