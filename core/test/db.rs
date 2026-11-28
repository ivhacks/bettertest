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
        worker: None,
        labels: vec!["x86".into()],
        image: None,
        vm: None,
        arch: None,
        timeout: None,
        sandbox: None,
    }
}

#[test]
fn a_rewound_task_is_pending_again_and_keeps_only_its_log() {
    let mut db = db();
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Test".into(),
            sandbox: None,
            tasks: vec![task("cut_short")],
        }],
        sandboxes: Default::default(),
    };
    let run = db
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .unwrap();
    let id = run.stages[0].tasks[0].id;
    db.start_task(id, "http://worker:7358", START).unwrap();
    db.set_tests(
        id,
        Tests {
            total: 3,
            failed: 1,
        },
    )
    .unwrap();
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 7,
    };
    db.add_artifact(id, &artifact).unwrap();
    let line = LogLine {
        stream: LogStream::Stdout,
        line: "half way".into(),
    };
    db.append_log(id, std::slice::from_ref(&line)).unwrap();
    assert_eq!(db.running_runs().unwrap(), [run.id]);

    db.rewind_task(id).unwrap();
    let task = &db.run(run.id).unwrap().unwrap().stages[0].tasks[0];
    let fresh = TaskRun {
        id,
        name: "cut_short".into(),
        state: TaskState::Pending,
        worker: None,
        started: None,
        ended: None,
        carried_from: None,
        tests: None,
    };
    assert_eq!(*task, fresh);
    // Running again, it may publish app.deb again.
    assert_eq!(db.artifacts(id).unwrap(), []);
    db.add_artifact(id, &artifact).unwrap();
    assert_eq!(db.log(id).unwrap(), [line]);
    assert_eq!(db.running_runs().unwrap(), [run.id]);
    db.set_run_state(run.id, RunState::Pass).unwrap();
    assert!(db.running_runs().unwrap().is_empty());
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
        db.append_log(id, std::slice::from_ref(&line)).unwrap();
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
    db.append_log(id, std::slice::from_ref(&line)).unwrap();
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

#[test]
fn test_counts_stay_with_their_task_and_carry_over() {
    let mut db = db();
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Test".into(),
            sandbox: None,
            tasks: vec![task("quick"), task("fts")],
        }],
        sandboxes: Default::default(),
    };
    let run = db
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .unwrap();
    let quick = run.stages[0].tasks[0].id;
    let tests = Tests {
        total: 1_031_284,
        failed: 3,
    };
    db.set_tests(quick, tests).unwrap();
    let run = db.run(run.id).unwrap().unwrap();
    assert_eq!(run.stages[0].tasks[0].tests, Some(tests));
    assert_eq!(run.stages[0].tasks[1].tests, None);
    let carried = BTreeMap::from([(
        ("Test".to_string(), "quick".to_string()),
        run.stages[0].tasks[0].clone(),
    )]);
    let restart = db.insert_run("app", &pipeline, None, &carried).unwrap();
    let restart = db.run(restart.id).unwrap().unwrap();
    assert_eq!(restart.stages[0].tasks[0].carried_from, Some(quick));
    assert_eq!(restart.stages[0].tasks[0].tests, Some(tests));
    assert_eq!(restart.stages[0].tasks[1].tests, None);
}

#[test]
fn each_user_comes_with_their_own_certificates_and_passkeys() {
    let db = db();
    let ada = db.add_user("ada", Role::Admin).unwrap();
    let bob = db.add_user("bob", Role::Read).unwrap();
    db.add_certificate(ada, "ada 1", "aa", "2028-01-01")
        .unwrap();
    db.add_certificate(ada, "ada 2", "ab", "2028-02-01")
        .unwrap();
    db.add_passkey(bob, "bob 1", "cred", &serde_json::json!({}))
        .unwrap();
    let users = db.users().unwrap();
    let names: Vec<&str> = users.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["logged out", "ada", "bob"]);
    let certs = |u: &User| -> Vec<String> { u.certs.iter().map(|c| c.label.clone()).collect() };
    let keys = |u: &User| -> Vec<String> { u.keys.iter().map(|k| k.label.clone()).collect() };
    assert_eq!(certs(&users[1]), ["ada 1", "ada 2"]);
    assert_eq!(users[1].certs[1].expires, "2028-02-01");
    assert!(keys(&users[1]).is_empty());
    assert!(certs(&users[2]).is_empty());
    assert_eq!(keys(&users[2]), ["bob 1"]);
    assert_eq!((users[1].certificates, users[2].passkeys), (2, 1));
    assert!(users[0].certs.is_empty() && users[0].keys.is_empty());
}
