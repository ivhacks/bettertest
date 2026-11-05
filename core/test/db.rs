// Tests of db.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;

const DAY: u64 = 24 * 60 * 60 * 1000;
// Some day in 2026. Retention only ever compares times, so any will do.
const START: u64 = 20_400 * DAY;

fn db() -> Db {
    prepare(Connection::open_in_memory().unwrap()).unwrap()
}

fn task(name: &str) -> Task {
    Task {
        name: name.into(),
        worker: Some("http://worker:7358".into()),
        labels: vec![],
        image: None,
        vm: None,
        arch: None,
        timeout: None,
        sandbox: None,
    }
}

#[test]
fn runs_a_dead_boss_left_end_at_its_last_heartbeat() {
    let mut db = db();
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Test".into(),
            sandbox: None,
            tasks: vec![task("running"), task("pending")],
        }],
        sandboxes: Default::default(),
    };
    let run = db
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .unwrap();
    let running = run.stages[0].tasks[0].id;
    db.start_task(running, "http://worker:7358", START).unwrap();
    // The boss beat a minute in, then died; the next one starts a day later.
    db.heartbeat("boss", START + 60_000).unwrap();
    assert_eq!(db.fail_abandoned_runs(START + DAY).unwrap(), 1);
    let run = db.run(run.id).unwrap().unwrap();
    assert_eq!(run.state, RunState::Fail);
    let tasks = &run.stages[0].tasks;
    assert_eq!(
        (tasks[0].state, tasks[0].ended),
        (TaskState::Fail, Some(START + 60_000))
    );
    assert_eq!(tasks[1].state, TaskState::Skipped);
    let log = db.log(running).unwrap();
    assert_eq!(
        log.last().map(|l| l.line.as_str()),
        Some(
            "the boss stopped while this task was running: its last heartbeat was at 2025-11-08 00:01:00 UTC"
        )
    );
}

/// A run of two tasks: `old` ended on day 0, `newer` on day 20.
fn two_tasks(db: &mut Db) -> (Uuid, Uuid) {
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Test".into(),
            sandbox: None,
            tasks: vec![task("old"), task("newer")],
        }],
        sandboxes: Default::default(),
    };
    let run = db
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .unwrap();
    let ids: Vec<Uuid> = run.stages[0].tasks.iter().map(|t| t.id).collect();
    for (id, ended) in [(ids[0], START), (ids[1], START + 20 * DAY)] {
        db.start_task(id, "http://worker:7358", ended - 1000)
            .unwrap();
        let line = LogLine {
            stream: LogStream::Stdout,
            line: "hello".into(),
        };
        db.append_log(id, &line).unwrap();
        let artifact = Artifact {
            name: "app.deb".into(),
            size: 3,
        };
        db.add_artifact(id, &artifact).unwrap();
        db.set_task_state(id, TaskState::Pass, ended).unwrap();
    }
    (ids[0], ids[1])
}

#[test]
fn logs_go_once_their_days_are_up() {
    let mut db = db();
    let (old, newer) = two_tasks(&mut db);
    let keep = 30 * DAY;

    // Day 29: nothing is 30 days old yet.
    let now = START + 29 * DAY;
    assert_eq!(db.expire_logs(now - keep, now, 100).unwrap(), 0);
    assert_eq!(db.log(old).unwrap().len(), 1);

    // Day 31: the task that ended on day 0 loses its log, the one from day 20 keeps it.
    let now = START + 31 * DAY;
    assert_eq!(db.expire_logs(now - keep, now, 100).unwrap(), 1);
    assert!(db.log(old).unwrap().is_empty());
    assert_eq!(db.log(newer).unwrap().len(), 1);
    assert_eq!(db.deleted(old).unwrap(), (Some(now), None));
    assert_eq!(db.deleted(newer).unwrap(), (None, None));

    // Done once: the next sweep has nothing left to do for it.
    assert_eq!(db.expire_logs(now - keep, now, 100).unwrap(), 0);

    // Day 51: the other one goes too.
    let now = START + 51 * DAY;
    assert_eq!(db.expire_logs(now - keep, now, 100).unwrap(), 1);
    assert!(db.log(newer).unwrap().is_empty());
}

#[test]
fn artifacts_go_once_their_days_are_up() {
    let mut db = db();
    let (old, newer) = two_tasks(&mut db);
    let keep = 7 * DAY;

    let now = START + 6 * DAY;
    assert!(db.expiring_artifacts(now - keep, 100).unwrap().is_empty());

    let now = START + 8 * DAY;
    let expiring = db.expiring_artifacts(now - keep, 100).unwrap();
    assert_eq!(expiring, [old]);
    db.forget_artifacts(&expiring, now).unwrap();
    assert!(db.artifacts(old).unwrap().is_empty());
    assert_eq!(db.artifacts(newer).unwrap().len(), 1);
    assert_eq!(db.deleted(old).unwrap(), (None, Some(now)));
    // Gone means gone for good: nothing is left to find.
    assert!(db.expiring_artifacts(now - keep, 100).unwrap().is_empty());
    let latest = db.latest_run_with_artifact("app", "Test", "old", "app.deb");
    assert_eq!(latest.unwrap(), None);
}

#[test]
fn running_tasks_are_never_expired() {
    let mut db = db();
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Test".into(),
            sandbox: None,
            tasks: vec![task("forever")],
        }],
        sandboxes: Default::default(),
    };
    let run = db
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .unwrap();
    let id = run.stages[0].tasks[0].id;
    db.start_task(id, "http://worker:7358", START).unwrap();
    let line = LogLine {
        stream: LogStream::Stdout,
        line: "still going".into(),
    };
    db.append_log(id, &line).unwrap();
    let now = START + 1000 * DAY;
    assert_eq!(db.expire_logs(now - DAY, now, 100).unwrap(), 0);
    assert_eq!(db.log(id).unwrap().len(), 1);
}

#[test]
fn settings_default_to_keeping_everything() {
    let mut db = db();
    assert_eq!(db.settings().unwrap(), Settings::default());
    let settings = Settings {
        artifact_days: Some(30),
        log_days: None,
    };
    db.set_settings(&settings).unwrap();
    assert_eq!(db.settings().unwrap(), settings);
    db.set_settings(&Settings::default()).unwrap();
    assert_eq!(db.settings().unwrap(), Settings::default());
}
