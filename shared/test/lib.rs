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
