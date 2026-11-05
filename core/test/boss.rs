// Tests of boss/mod.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use std::collections::BTreeSet;

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

// Build (compile), Test (unit, e2e), Deploy (shared sandbox: push, smoke).
fn pipeline() -> Pipeline {
    let stage = |name: &str, tasks: &[&str]| Stage {
        name: name.into(),
        sandbox: None,
        tasks: tasks.iter().map(|t| task(t)).collect(),
    };
    let mut deploy = stage("Deploy", &["push", "smoke"]);
    deploy.sandbox = Some(Sandbox {
        worker: Some("http://worker:7358".into()),
        labels: vec![],
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
        worker: None,
        ..task(name)
    };
    let stage = |name: &str, tasks: Vec<Task>| Stage {
        name: name.into(),
        sandbox: None,
        tasks,
    };
    let vm = |image: &str| Sandbox {
        worker: Some("http://worker:7358".into()),
        labels: vec![],
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

fn picked(up: &BTreeMap<String, Watched>, named: Option<&str>, labels: &[&str]) -> String {
    let labels: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
    match pick_worker(up, named, &labels) {
        Ok(Pick::Worker(url)) => url,
        Ok(Pick::Busy(why)) => format!("busy: {why}"),
        Err(why) => format!("none: {why}"),
    }
}

#[test]
fn picks_the_least_busy_worker() {
    // b runs fewer tasks of the four each takes.
    let up = workers(&[("http://a", &["x86"], 2, 4), ("http://b", &["x86"], 1, 4)]);
    assert_eq!(picked(&up, None, &["x86"]), "http://b");
    // Equally busy: either, and over many picks both.
    let up = workers(&[
        ("http://a", &["x86"], 0, 1),
        ("http://b", &["x86"], 0, 1),
        ("http://c", &["x86"], 1, 2),
    ]);
    let all: BTreeSet<String> = (0..100).map(|_| picked(&up, None, &["x86"])).collect();
    assert_eq!(all, ["http://a".to_string(), "http://b".to_string()].into());
    // Only workers with every label count.
    let up = workers(&[
        ("http://a", &["x86"], 0, 1),
        ("http://b", &["x86", "gpu"], 0, 1),
    ]);
    assert_eq!(picked(&up, None, &["x86", "gpu"]), "http://b");
}

#[test]
fn a_full_worker_gets_no_more_tasks() {
    // a takes one task and runs one; b takes two and runs one.
    let up = workers(&[("http://a", &["x86"], 1, 1), ("http://b", &["x86"], 1, 2)]);
    assert_eq!(picked(&up, None, &["x86"]), "http://b");
    let up = workers(&[("http://a", &["x86"], 1, 1), ("http://b", &["x86"], 2, 2)]);
    assert_eq!(
        picked(&up, None, &["x86"]),
        "busy: every worker with labels x86 is running as many tasks as it takes"
    );
    assert_eq!(
        picked(&up, Some("http://a"), &[]),
        "busy: http://a is running as many tasks as it takes"
    );
    // A worker the boss does not know yet is tried, and the task says how that went.
    assert_eq!(picked(&up, Some("http://c"), &[]), "http://c");
    // One it knows but is still connecting to is waited for: how many tasks it takes is
    // not known yet. So are labels some worker still connecting may have.
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
        picked(&up, Some("http://d"), &[]),
        "busy: http://d is connecting"
    );
    assert_eq!(
        picked(&up, None, &["arm"]),
        "busy: no connected worker has labels arm yet, and some are still connecting"
    );
    // One the boss could not reach is tried, and the task says why it failed.
    if let Some(d) = up.get_mut("http://d") {
        d.worker.problem = Some("http://d did not answer".into());
    }
    assert_eq!(picked(&up, Some("http://d"), &[]), "http://d");
    // Busy or not, no worker with the labels at all is an error at once.
    assert!(picked(&up, None, &["arm"]).starts_with("none: no connected worker has labels arm"));
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
        picked(&up, None, &["x86", "gpuu"]),
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
