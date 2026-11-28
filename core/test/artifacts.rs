// Tests of boss/artifacts.rs: uploads, and why a task gets no artifact when it asks for one. A child
// of the boss's tests (see the #[path] in test/boss.rs), so it reaches what the boss keeps private.

use super::*;

/// A run of pipeline() in a fresh boss's db, under `name`.
fn inserted(boss: &Boss, name: &str) -> Run {
    let mut db = boss.db();
    db.insert_run(name, &pipeline(), None, &BTreeMap::new())
        .unwrap()
}

fn task_id(run: &Run, stage: &str, task: &str) -> Uuid {
    let stage = run.stages.iter().find(|s| s.name == stage).unwrap();
    stage.tasks.iter().find(|t| t.name == task).unwrap().id
}

#[tokio::test]
async fn an_upload_is_there_once_and_never_replaced() {
    let name = format!("upload-{}.qcow2", Uuid::new_v4().simple());
    let path = AxumPath(name.clone());
    let put = |body: &'static str| put_artifact(AxumPath(name.clone()), Body::from(body));
    assert_eq!(put("first").await, Ok(StatusCode::CREATED));
    let again = put("second").await.unwrap_err();
    assert_eq!(
        again,
        (
            StatusCode::CONFLICT,
            format!("artifact {name} already exists")
        )
    );
    let got = get_artifact(path).await.unwrap();
    assert_eq!(got.headers()[header::CONTENT_LENGTH], "5");
    let body = axum::body::to_bytes(got.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body, "first");
    // Nothing of either upload is left beside it.
    let dir = workdir::dir("artifacts").unwrap();
    let partial = |entry: std::io::Result<fs::DirEntry>| {
        let file = entry.unwrap().file_name().to_string_lossy().into_owned();
        file.starts_with(&format!(".{name}."))
    };
    assert!(!fs::read_dir(&dir).unwrap().any(partial));
    fs::remove_file(dir.join(&name)).unwrap();

    let missing = get_artifact(AxumPath(format!("never-{name}"))).await;
    let missing = missing.map(|_| ()).unwrap_err();
    assert_eq!(missing, not_found(format!("artifact never-{name}")));
    // A name that could reach outside the artifacts, or hide in them, is not one.
    let rule = "artifact names are letters, digits, '.', '-' and '_', not starting with '.'";
    for bad in ["../bettertest.db", ".hidden", "a b", ""] {
        let refused = put_artifact(AxumPath(bad.into()), Body::from("x")).await;
        assert_eq!(refused, Err(bad_request(rule)), "{bad:?}");
    }
}

#[test]
fn a_producer_that_has_not_published_yet_says_why() {
    let boss = boss(BTreeMap::new());
    let run = inserted(&boss, "app");
    let find = |this_run, task| {
        boss.find_artifact(&run, this_run, "Test", task, "app.deb")
            .unwrap_err()
    };
    let short = run.short_id();
    assert_eq!(
        find(true, "unit"),
        "Can't get artifact app.deb of Test.unit because Test.unit hasn't run yet. Move Test.unit \
         to a stage that runs before this one for the artifact to be available."
    );
    assert_eq!(
        find(false, "unit"),
        format!(
            "Can't get artifact app.deb of Test.unit because Test.unit hasn't run yet in run \
             {short} of app."
        )
    );
    let set = |task, state| {
        let id = task_id(&run, "Test", task);
        boss.db().set_task_state(id, state, 1).unwrap();
    };
    set("unit", TaskState::Running);
    assert_eq!(
        find(true, "unit"),
        "Can't get artifact app.deb of Test.unit because Test.unit is still running. Move \
         Test.unit to a stage that runs before this one for the artifact to be available."
    );
    set("e2e", TaskState::Skipped);
    assert_eq!(
        find(false, "e2e"),
        format!(
            "Can't get artifact app.deb of Test.e2e because Test.e2e was skipped in run {short} \
             of app, so it published nothing."
        )
    );
    assert_eq!(
        find(true, "lint"),
        "Can't get artifact app.deb of Test.lint: this run has no task Test.lint."
    );

    // Retention got there first: said so, rather than that it never existed.
    set("unit", TaskState::Pass);
    let unit = task_id(&run, "Test", "unit");
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 3,
    };
    boss.db().add_artifact(unit, &artifact).unwrap();
    assert!(
        boss.find_artifact(&run, true, "Test", "unit", "app.deb")
            .is_ok()
    );
    boss.db().forget_artifacts(&[unit], 2).unwrap();
    assert_eq!(
        find(false, "unit"),
        format!(
            "Can't get artifact app.deb of Test.unit: retention deleted the artifacts of \
             Test.unit in run {short} of app. The settings page says how long they are kept."
        )
    );
}

#[test]
fn a_carried_task_hands_over_what_the_run_that_ran_it_published() {
    let boss = boss(BTreeMap::new());
    let earlier = inserted(&boss, "app");
    let compile = task_id(&earlier, "Build", "compile");
    boss.db()
        .set_task_state(compile, TaskState::Pass, 1)
        .unwrap();
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 3,
    };
    boss.db().add_artifact(compile, &artifact).unwrap();
    let earlier = boss.db().run(earlier.id).unwrap().unwrap();
    let carried = carry_over(&earlier, &pipeline(), Some("Test"), None).unwrap();
    let restart = (boss.db())
        .insert_run("app", &pipeline(), None, &carried)
        .unwrap();
    let found = boss.find_artifact(&restart, true, "Build", "compile", "app.deb");
    assert_eq!(
        found,
        Ok(task_artifacts_dir(compile).unwrap().join("app.deb"))
    );
}

#[test]
fn a_run_is_named_by_latest_or_by_its_id() {
    let boss = boss(BTreeMap::new());
    let resolve =
        |pipeline: &str, run: &str| boss.resolve_run(pipeline, run, "Build", "compile", "app.deb");
    let cant = "Can't get artifact app.deb of app Build.compile";
    assert_eq!(
        resolve("app", "latest"),
        Err(
            "Can't get artifact app.deb of app Build.compile: there is no pipeline named 'app'."
                .into()
        )
    );
    // Seventeen runs: two of them start with the same hex digit.
    let runs: Vec<Run> = (0..17).map(|_| inserted(&boss, "app")).collect();
    assert_eq!(
        resolve("app", "latest"),
        Err(format!(
            "{cant}: no run of app has Build.compile passing with an artifact named 'app.deb'."
        ))
    );
    for bad in ["", "LATEST!", "6dfde7x", &"a".repeat(33)] {
        assert_eq!(
            resolve("app", bad),
            Err(format!(
                "{cant}: '{bad}' is not a run id. Use LATEST, or a run's id like 6dfde7."
            )),
            "{bad:?}"
        );
    }

    // Whole, with or without dashes, in either case, or as much of it as tells it apart.
    let one = &runs[3];
    let simple = one.id.simple().to_string();
    for id in [
        one.id.to_string(),
        simple.clone(),
        simple.to_ascii_uppercase(),
        simple[..12].to_string(),
    ] {
        assert_eq!(resolve("app", &id).map(|r| r.id), Ok(one.id), "{id}");
    }
    // An id of another pipeline's run is not one of this pipeline's.
    let other = inserted(&boss, "web");
    let other_id = other.id.simple().to_string();
    assert_eq!(
        resolve("app", &other_id),
        Err(format!("{cant}: app has no run {other_id}."))
    );

    let first = |run: &Run| run.id.simple().to_string().chars().next().unwrap();
    let digit = (runs.iter().map(first))
        .find(|d| runs.iter().filter(|r| first(r) == *d).count() > 1)
        .unwrap();
    let mut sharing: Vec<String> = (runs.iter().rev())
        .filter(|r| first(r) == digit)
        .map(|r| r.id.simple().to_string()[..12].to_string())
        .collect();
    sharing.truncate(10);
    let prefix = digit.to_string();
    assert_eq!(
        resolve("app", &prefix),
        Err(format!(
            "{cant}: run id {prefix} could be any of {}. Use more of it.",
            sharing.join(", ")
        ))
    );

    // Latest is the newest run in which the task passed and published it.
    for run in [&runs[5], &runs[9]] {
        let compile = task_id(run, "Build", "compile");
        boss.db()
            .set_task_state(compile, TaskState::Pass, 1)
            .unwrap();
        let artifact = Artifact {
            name: "app.deb".into(),
            size: 3,
        };
        boss.db().add_artifact(compile, &artifact).unwrap();
    }
    let failed = task_id(&runs[12], "Build", "compile");
    boss.db()
        .set_task_state(failed, TaskState::Fail, 1)
        .unwrap();
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 3,
    };
    boss.db().add_artifact(failed, &artifact).unwrap();
    assert_eq!(resolve("app", "latest").map(|r| r.id), Ok(runs[9].id));
}
