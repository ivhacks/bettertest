// Tests of boss/mod.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
};

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
                        tests: None,
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
fn restarting_a_task_carries_over_what_came_before_and_its_siblings() {
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
             Restart Test instead."
                .to_string()
        )
    );
}

#[test]
fn a_task_in_a_shared_sandbox_restarts_its_whole_stage() {
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
fn a_task_in_a_parallel_shared_sandbox_restarts_alone() {
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
fn restarting_a_stage_that_uses_a_spanning_sandbox_restarts_from_its_first_stage() {
    let pipeline = spanning_pipeline();
    // Reboot needs DEBIAN and FEDORA as Install left them: Install restarts too.
    assert_eq!(restart_start(&pipeline, 2, None), (1, None));
    assert_eq!(restart_start(&pipeline, 2, Some("debian")), (1, None));
    // Install is where they start, but a task in a sandbox restarts its whole stage.
    assert_eq!(restart_start(&pipeline, 1, Some("debian")), (1, None));
    // Check and Build use no sandbox: as before.
    assert_eq!(restart_start(&pipeline, 3, Some("web")), (3, Some("web")));
    assert_eq!(restart_start(&pipeline, 0, None), (0, None));
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
fn an_interrupted_run_goes_on_from_its_first_stage_not_passed() {
    use TaskState::{Pending, Running};
    let interrupted = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass), ("e2e", Running)]),
        ("Deploy", &[("push", Pending), ("smoke", Pending)]),
    ]);
    assert_eq!(resume_from(&interrupted, &pipeline()), Some(1));
    // Cut short, its tasks were skipped: the same.
    let interrupted = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Skipped), ("e2e", Skipped)]),
        ("Deploy", &[("push", Skipped), ("smoke", Skipped)]),
    ]);
    assert_eq!(resume_from(&interrupted, &pipeline()), Some(1));
    // A failed task failed the run, restart or not. Every task passed: nothing is left to run.
    let failed = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Fail), ("e2e", Running)]),
        ("Deploy", &[("push", Pending), ("smoke", Pending)]),
    ]);
    assert_eq!(resume_from(&failed, &pipeline()), None);
    let passed = run(&[
        ("Build", &[("compile", Pass)]),
        ("Test", &[("unit", Pass), ("e2e", Pass)]),
        ("Deploy", &[("push", Pass), ("smoke", Pass)]),
    ]);
    assert_eq!(resume_from(&passed, &pipeline()), None);
    // Reboot needs DEBIAN and FEDORA as Install left them, gone with the restart: from Install.
    let interrupted = run(&[
        ("Build", &[("compile", Pass)]),
        ("Install", &[("debian", Pass), ("fedora", Pass)]),
        ("Reboot", &[("debian", Pass), ("fedora", Running)]),
        ("Check", &[("web", Pending)]),
    ]);
    assert_eq!(resume_from(&interrupted, &spanning_pipeline()), Some(1));
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
    match pick_worker(up, named, &labels, now(), &[]) {
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
fn a_worker_that_just_went_away_is_waited_for() {
    // d was last heard from a moment ago and may be restarting: tasks wait for it rather than fail.
    let mut up = workers(&[("http://arm", &["arm"], 0, 1)]);
    let gone = Worker {
        url: "http://d".into(),
        info: None,
        since: now() - 10_000,
        problem: Some("http://d is restarting".into()),
        tasks: vec![],
    };
    let watched = Watched {
        worker: gone,
        watcher: None,
    };
    up.insert("http://d".into(), watched);
    assert_eq!(
        picked(&up, &["x86"]),
        "busy: no connected worker has labels x86 yet, and some are still connecting"
    );
    assert_eq!(
        picked_on(&up, Some("http://d"), &[]),
        "busy: http://d is connecting"
    );
    // Until COMEBACK after that. Then it is gone for good, as far as tasks are concerned.
    let d = up.get("http://d").expect("d");
    assert_eq!(back_by(d), Some(d.worker.since + COMEBACK));
    if let Some(d) = up.get_mut("http://d") {
        d.worker.since = now() - COMEBACK;
    }
    let got = picked(&up, &["x86"]);
    assert!(
        got.starts_with("none: no connected worker has labels x86"),
        "{got}"
    );
    assert_eq!(picked_on(&up, Some("http://d"), &[]), "http://d");
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
    // Short labels may be one letter off, longer ones two.
    assert!(similar_label("gpu", "cpu"));
    assert!(!similar_label("gpu", "cpx"));
    assert!(!similar_label("gpus", "cpu"));
    assert!(similar_label("gpus", "cpus1"));
    assert!(similar_label("fixture", "fixtrue"));
    assert!(!similar_label("fixture", "fxtrue1"));
    assert_eq!(
        picked(&BTreeMap::new(), &["gpu"]),
        "none: no connected worker has labels gpu. Connected workers: none. The workers page \
         shows them all."
    );
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

// Stand-ins for frontend/dist and the gzip copies build.rs makes of it. Each says which it is.
#[derive(Embed)]
#[folder = "test/web/"]
struct WebFixture;

/// The headers that matter and the body of serving `path` to a browser that sends `accept`.
async fn serve(path: &str, accept: Option<&str>) -> (StatusCode, BTreeMap<String, String>, String) {
    let mut headers = HeaderMap::new();
    if let Some(accept) = accept {
        headers.insert(header::ACCEPT_ENCODING, accept.parse().unwrap());
    }
    let response = embedded::<WebFixture>(&headers, path);
    let status = response.status();
    let mut seen = BTreeMap::new();
    for name in ["content-type", "content-encoding", "cache-control", "vary"] {
        if let Some(value) = response.headers().get(name) {
            seen.insert(name.to_string(), value.to_str().unwrap().to_string());
        }
    }
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, seen, String::from_utf8(body.to_vec()).unwrap())
}

fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn shell_files_are_gzipped_when_the_browser_takes_gzip() {
    let forever = "public, max-age=31536000, immutable";
    let js = |encoding: Option<&str>| {
        let mut pairs = vec![
            ("content-type", "text/javascript"),
            ("cache-control", forever),
            ("vary", "accept-encoding"),
        ];
        pairs.extend(encoding.map(|e| ("content-encoding", e)));
        headers(&pairs)
    };
    let ok = StatusCode::OK;
    let gzip = (ok, js(Some("gzip")), "app.js in gzip\n".to_string());
    let plain = (ok, js(None), "app.js as it is\n".to_string());
    // Chrome over HTTPS, then over plain HTTP.
    assert_eq!(serve("app.js", Some("gzip, deflate, br, zstd")).await, gzip);
    assert_eq!(serve("app.js", Some("gzip, deflate")).await, gzip);
    assert_eq!(serve("app.js", Some("br, GZip;q=0.5")).await, gzip);
    assert_eq!(serve("app.js", Some("br, gzip; Q=0")).await, plain);
    assert_eq!(serve("app.js", Some("br, identity")).await, plain);
    assert_eq!(serve("app.js", None).await, plain);

    // No compressed copy to choose from: as it is, with nothing to vary on.
    let svg = headers(&[
        ("content-type", "image/svg+xml"),
        ("cache-control", forever),
    ]);
    let mark = "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n".to_string();
    assert_eq!(serve("mark.svg", Some("gzip")).await, (ok, svg, mark));

    // The document names the hashed files, so the browser checks it every time.
    let document = headers(&[
        ("content-type", "text/html"),
        ("content-encoding", "gzip"),
        ("cache-control", "no-cache"),
        ("vary", "accept-encoding"),
    ]);
    let html = "index.html in gzip\n".to_string();
    assert_eq!(
        serve("index.html", Some("gzip")).await,
        (ok, document, html)
    );

    // wasm-bindgen names a snippet after its crate, not its code, so it is checked too.
    let snippet = headers(&[
        ("content-type", "text/javascript"),
        ("cache-control", "no-cache"),
    ]);
    let plot = "export function plot() {}\n".to_string();
    let inline = serve("snippets/app-0123/inline0.js", Some("gzip")).await;
    assert_eq!(inline, (ok, snippet, plot));

    let text = headers(&[("content-type", "text/plain; charset=utf-8")]);
    let missing = serve("app.js.map", Some("gzip")).await;
    assert_eq!(missing, (StatusCode::NOT_FOUND, text, "not found".into()));
}

#[test]
fn home_names_the_first_pipeline_as_canonical() {
    let index = b"<html><head><title>bettertest</title></head><body>bettertest</body></html>";
    assert_eq!(
        with_canonical(index, "app"),
        "<html><head><title>bettertest</title>\
         <link rel=\"canonical\" href=\"/pipeline/app\"></head><body>bettertest</body></html>"
    );
}

#[tokio::test]
async fn the_home_document_is_gzipped_when_the_browser_takes_gzip() {
    let html = with_canonical(
        b"<html><head><title>bettertest</title></head><body>frame</body></html>",
        "app",
    );
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT_ENCODING, "gzip".parse().unwrap());
    let response = document(&headers, html.clone());
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(response.headers()[header::VARY], "accept-encoding");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut decoded = flate2::write::GzDecoder::new(Vec::new());
    std::io::Write::write_all(&mut decoded, &body).unwrap();
    let text = String::from_utf8(decoded.finish().unwrap()).unwrap();
    assert_eq!(text, html);

    headers.insert(header::ACCEPT_ENCODING, "gzip;q=0".parse().unwrap());
    let plain = document(&headers, html.clone());
    assert!(plain.headers().get(header::CONTENT_ENCODING).is_none());
    assert!(plain.headers().get(header::VARY).is_none());
    assert_eq!(plain.headers()[header::CACHE_CONTROL], "no-cache");
    let body = axum::body::to_bytes(plain.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body, html);
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
        parsed: Mutex::new(HashMap::new()),
        shown: Mutex::new(HashMap::new()),
        repos: vec![],
        going: Mutex::new(HashMap::new()),
        db: Mutex::new(db),
        workers: Mutex::new(up),
        task_tokens: Mutex::new(HashMap::new()),
        ceremonies: Mutex::new(HashMap::new()),
        events: broadcast::channel(4096).0,
        freed: Notify::new(),
        waiting: Mutex::new(BTreeMap::new()),
        http: client::http().unwrap(),
        tls: None,
        port: LISTEN_PORT,
    })
}

/// A worker that runs every task it is sent in a moment.
async fn quick_worker() -> String {
    worker_saying("event: exit\ndata: 0\n\n").await
}

/// A worker that answers every task it is sent with `stream`, in a moment.
async fn worker_saying(stream: &'static str) -> String {
    let start_task = move || async move {
        sleep(Duration::from_millis(5)).await;
        ([(header::CONTENT_TYPE, "text/event-stream")], stream)
    };
    let app = Router::new().route("/start-task", post(start_task));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// Runs a `parallel` stage of tasks t0 to t39 on a worker that takes two at once, restarting task
/// `restart` of `earlier` if given: whether it passed, and the tasks it started, in order.
async fn parallel_stage(restart: Option<(&Run, &str)>) -> (bool, Vec<String>) {
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
    let carried = match restart {
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
async fn restarting_a_task_of_a_parallel_stage_runs_only_that_task() {
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

#[tokio::test(flavor = "multi_thread")]
async fn test_counts_a_task_reports_are_kept_and_sent_live() {
    let url = worker_saying(concat!(
        "event: tests\ndata: {\"total\": 1234, \"failed\": 2}\n\n",
        "event: exit\ndata: 1\n\n",
    ))
    .await;
    let boss = boss(workers(&[(&url, &["x86"], 0, 1)]));
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
        tasks: vec![task("fts")],
    };
    let pipeline = Pipeline {
        stages: vec![stage.clone()],
        sandboxes: Default::default(),
    };
    let run = boss
        .db()
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .unwrap();
    let r = Running {
        boss: boss.clone(),
        run: Arc::new(run.clone()),
        source: "".into(),
        cancelled: watch::channel(None).1,
        spanning: Spanning::default(),
    };
    assert!(!run_shared_stage(&r, &stage, &sandbox, &run.stages[0]).await);
    let tests = Tests {
        total: 1234,
        failed: 2,
    };
    let fts = run.stages[0].tasks[0].id;
    let mut sent = vec![];
    while let Ok(event) = events.try_recv() {
        if let Event::TaskTests { task, tests, .. } = event {
            sent.push((task, tests));
        }
    }
    assert_eq!(sent, [(fts, tests)]);
    let kept = boss.db().run(run.id).unwrap().unwrap();
    assert_eq!(kept.stages[0].tasks[0].tests, Some(tests));
    assert_eq!(kept.stages[0].tasks[0].state, Fail);
}

/// A worker whose `hang`th start-task (counting from 1) prints "first try" and then never ends.
/// The rest exit 0 at once. `calls` counts them all.
async fn worker_hanging_once(hang: usize, calls: Arc<AtomicUsize>) -> String {
    let start_task = move || async move {
        let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
        let body = if call == hang {
            let line = Ok::<_, Infallible>("event: stdout\ndata: first try\n\n");
            Body::from_stream(tokio_stream::StreamExt::chain(
                tokio_stream::once(line),
                tokio_stream::pending(),
            ))
        } else {
            Body::from("event: exit\ndata: 0\n\n")
        };
        ([(header::CONTENT_TYPE, "text/event-stream")], body)
    };
    let app = Router::new().route("/start-task", post(start_task));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// A boss that knows the worker at `url`, and pipeline app: stage First (task once), then Second
/// (task twice), both on that worker. With the pipeline, parsed.
async fn two_stages_on(url: &str) -> (Shared, Pipeline, String) {
    let source = format!(
        "from bettertest import Stage, task\n\n\nclass First(Stage):\n    worker = \"{url}\"\n\n    \
         @task()\n    def once():\n        pass\n\n\nclass Second(Stage):\n    worker = \"{url}\"\n\n    \
         @task()\n    def twice():\n        pass\n"
    );
    let file = std::env::temp_dir().join(format!("app-{}.py", Uuid::new_v4()));
    std::fs::write(&file, &source).expect("write pipedef");
    let boss = boss(workers(&[(url, &["x86"], 0, 1)]));
    lock(&boss.pipedefs).insert("app".into(), Pipedef::File(file));
    let (source, pipeline) = runs::load_pipeline(&boss, "app", None).await.expect("load");
    (boss, pipeline.expect("parses"), source)
}

/// The state run `id` ends in, from `events`.
async fn finished(events: &mut broadcast::Receiver<Event>, id: Uuid) -> RunState {
    let ended = async {
        loop {
            if let Ok(Event::RunFinished { run, state, .. }) = events.recv().await
                && run == id
            {
                return state;
            }
        }
    };
    timeout(Duration::from_secs(20), ended)
        .await
        .expect("the run ends")
}

fn log_text(boss: &Boss, task: Uuid) -> Vec<String> {
    let log = boss.db().log(task).expect("log");
    log.into_iter().map(|line| line.line).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_cut_short_by_a_restarting_worker_goes_on_and_passes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let url = worker_hanging_once(2, calls.clone()).await;
    let (boss, pipeline, source) = two_stages_on(&url).await;
    let mut events = boss.events.subscribe();
    let loaded = (source, Ok(pipeline));
    let headers = HeaderMap::new();
    let run = begin_run(&boss, &headers, "app", loaded, None, BTreeMap::new()).expect("run");
    // Second.twice is running when the worker says it is stopping.
    let running = async {
        loop {
            if let Ok(Event::Log { line, .. }) = events.recv().await
                && line.line == "first try"
            {
                return;
            }
        }
    };
    timeout(Duration::from_secs(20), running)
        .await
        .expect("twice runs");
    boss.worker_stopping(&url);
    assert_eq!(finished(&mut events, run.id).await, RunState::Pass);
    // First.once ran once. Second.twice was cut short, and ran again.
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let twice = run.stages[1].tasks[0].id;
    let restarting = format!("{url} is restarting");
    assert_eq!(
        log_text(&boss, twice),
        [
            "first try",
            &restarting,
            &format!("{restarting}: running this again"),
            "exit code 0"
        ]
    );
    let kept = boss.db().run(run.id).expect("db").expect("run");
    assert_eq!(kept.state, RunState::Pass);
    let states: Vec<TaskState> = kept.stages.iter().map(|s| s.tasks[0].state).collect();
    assert_eq!(states, [Pass, Pass]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_the_previous_boss_left_going_goes_on_and_passes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let url = worker_hanging_once(0, calls.clone()).await;
    let (boss, pipeline, _) = two_stages_on(&url).await;
    // As a boss that stopped while Second.twice ran left it.
    let run = boss
        .db()
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .expect("run");
    let (once, twice) = (run.stages[0].tasks[0].id, run.stages[1].tasks[0].id);
    {
        let db = boss.db();
        db.start_task(once, &url, 1).expect("start");
        db.set_task_state(once, Pass, 2).expect("pass");
        db.start_task(twice, &url, 3).expect("start");
    }
    assert_eq!(boss.db().running_runs().expect("db"), [run.id]);
    let mut events = boss.events.subscribe();
    spawn(resume(
        boss.clone(),
        run.id,
        "the boss restarted".into(),
        None,
    ));
    assert_eq!(finished(&mut events, run.id).await, RunState::Pass);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "only Second.twice ran");
    assert_eq!(
        log_text(&boss, twice),
        ["the boss restarted: running this again", "exit code 0"]
    );
    assert!(boss.db().running_runs().expect("db").is_empty());
}

/// A restart cut Second.twice short, as the boss dying leaves it (running) or as a restarting
/// worker does (skipped once it had started), and the pipedef has changed since, so the run cannot
/// go on. It fails, and so does the task the restart cut short, its log saying why: a failed run
/// shows which task it failed at.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_cannot_go_on_after_a_restart_fails_the_task_it_cut_short() {
    let url = worker_hanging_once(0, Arc::new(AtomicUsize::new(0))).await;
    let (boss, pipeline, source) = two_stages_on(&url).await;
    let Some(Pipedef::File(file)) = boss.pipedef("app") else {
        panic!("app is a file");
    };
    std::fs::write(&file, source.replace("Second", "Renamed")).expect("edit the pipedef");
    for cut_short in [TaskState::Running, Skipped] {
        let run = boss
            .db()
            .insert_run("app", &pipeline, None, &BTreeMap::new())
            .expect("run");
        let (once, twice) = (run.stages[0].tasks[0].id, run.stages[1].tasks[0].id);
        {
            let db = boss.db();
            db.start_task(once, &url, 1).expect("start");
            db.set_task_state(once, Pass, 2).expect("pass");
            db.start_task(twice, &url, 3).expect("start");
            if cut_short == Skipped {
                db.set_task_state(twice, Skipped, 4).expect("skip");
            }
        }
        let mut events = boss.events.subscribe();
        spawn(resume(
            boss.clone(),
            run.id,
            "the boss restarted".into(),
            None,
        ));
        assert_eq!(finished(&mut events, run.id).await, RunState::Fail);
        let kept = boss.db().run(run.id).expect("db").expect("run");
        let states: Vec<TaskState> = kept.stages.iter().map(|s| s.tasks[0].state).collect();
        assert_eq!(states, [Pass, Fail], "cut short as {cut_short:?}");
        assert_eq!(
            log_text(&boss, twice),
            [
                "the boss restarted, and the run cannot go on: its pipedef no longer has the stages \
              and tasks it had"
            ]
        );
    }
}

/// A pipeline of one stage, Build, with one task, compile, that needs label x86.
fn one_task() -> Pipeline {
    Pipeline {
        stages: vec![Stage {
            name: "Build".into(),
            sandbox: None,
            tasks: vec![task("compile")],
        }],
        sandboxes: Default::default(),
    }
}

/// When the worker's one slot frees, the oldest run waiting for it gets it, even when a newer run
/// asks before that run's task has woken up. Otherwise a run the boss went on with after a restart
/// waits behind every run started since, its tasks pending for hours.
#[tokio::test]
async fn a_freed_slot_goes_to_the_oldest_run_waiting_for_it() {
    // The worker takes one task, and runs one.
    let boss = boss(workers(&[("http://w", &["x86"], 1, 1)]));
    let insert = || {
        let none = BTreeMap::new();
        boss.db()
            .insert_run("app", &one_task(), None, &none)
            .expect("run")
    };
    let (older, newer) = (insert(), insert());
    let compile = |run: &Run| run.stages[0].tasks[0].id;
    let (_cancel, cancelled) = watch::channel(None);
    let labels = vec!["x86".to_string()];

    let mut events = boss.events.subscribe();
    let waiting = {
        let (boss, older, labels, cancelled) = (
            boss.clone(),
            older.clone(),
            labels.clone(),
            cancelled.clone(),
        );
        spawn(async move {
            let task = compile(&older);
            boss.claim_worker(&older, task, None, &labels, &cancelled)
                .await
        })
    };
    let says_waiting = async {
        loop {
            if let Ok(Event::Log { task, line }) = events.recv().await
                && task == compile(&older)
                && line.line.starts_with("waiting")
            {
                return;
            }
        }
    };
    timeout(Duration::from_secs(5), says_waiting)
        .await
        .expect("the older run's task waits");

    // The slot frees, and the newer run asks for it before the older one's task has run again.
    boss.task_ended(Uuid::nil());
    let asking = boss.claim_worker(&newer, compile(&newer), None, &labels, &cancelled);
    let got = tokio::select! {
        biased;
        got = asking => Some(got),
        () = std::future::ready(()) => None,
    };
    assert_eq!(got, None, "the newer run waits");
    let got = timeout(Duration::from_secs(5), waiting).await;
    assert_eq!(
        got.expect("in time").expect("joined"),
        Ok("http://w".into())
    );
}

#[tokio::test]
async fn an_html_artifact_shows_sandboxed_and_the_rest_downloads() {
    let dir = artifacts::task_artifacts_dir(Uuid::new_v4()).expect("dir");
    std::fs::create_dir_all(&dir).expect("dir");
    for name in ["report.html", "app.deb"] {
        std::fs::write(dir.join(name), "<script>alert(1)</script>").expect("write");
    }
    let headers = |response: Response| {
        let header = |name| {
            response
                .headers()
                .get(name)
                .map(|v| v.to_str().expect("ascii").to_string())
        };
        (
            header(header::CONTENT_TYPE),
            header(header::CONTENT_SECURITY_POLICY),
        )
    };
    let sandbox = Some("sandbox allow-scripts".to_string());
    let html = artifacts::file_response(&dir.join("report.html"), "report.html", None).await;
    let html = headers(html.expect("html"));
    assert_eq!(
        html,
        (
            Some("text/html; charset=utf-8".to_string()),
            sandbox.clone()
        )
    );
    let deb = artifacts::file_response(&dir.join("app.deb"), "app.deb", None).await;
    let deb = headers(deb.expect("deb"));
    assert_eq!(
        deb,
        (
            Some("application/octet-stream".to_string()),
            sandbox.clone()
        )
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// A task's artifact downloads under its name, and fetched by pipeline names the run it came from.
#[tokio::test]
async fn an_artifact_names_its_file_and_its_run() {
    let source = big_pipedef(1, 1);
    let file = std::env::temp_dir().join(format!("artifact-{}.py", Uuid::new_v4()));
    std::fs::write(&file, &source).expect("write pipedef");
    let boss = boss(BTreeMap::new());
    let name = format!("artifact-{}", Uuid::new_v4().simple());
    lock(&boss.pipedefs).insert(name.clone(), Pipedef::File(file.clone()));
    let (_, pipeline) = runs::load_pipeline(&boss, &name, None).await.expect("load");
    let run = boss
        .db()
        .insert_run(&name, &pipeline.expect("parses"), None, &BTreeMap::new())
        .expect("run");
    let task = run.stages[0].tasks[0].id;
    boss.db()
        .set_task_state(task, TaskState::Pass, 1)
        .expect("pass");
    let dir = artifacts::task_artifacts_dir(task).expect("dir");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("app.deb"), "deb").expect("write");
    let artifact = Artifact {
        name: "app.deb".into(),
        size: 3,
    };
    boss.db().add_artifact(task, &artifact).expect("artifact");
    let header = |response: &Response, name: &str| {
        let value = response.headers().get(name).map(|v| v.to_str().unwrap());
        value.map(str::to_string)
    };

    let path = AxumPath((task.to_string(), "app.deb".to_string()));
    let download = artifacts::get_task_artifact(path).await.expect("download");
    let disposition = header(&download, "content-disposition");
    assert_eq!(
        disposition.as_deref(),
        Some("attachment; filename=\"app.deb\"")
    );
    assert_eq!(header(&download, "content-length").as_deref(), Some("3"));

    let path = [name.as_str(), "latest", "Stage0", "task_0_0", "app.deb"].map(str::to_string);
    let fetched = artifacts::get_pipeline_artifact(State(boss.clone()), AxumPath(path))
        .await
        .expect("fetch");
    let id = run.id.to_string();
    assert_eq!(
        header(&fetched, "bettertest-run").as_deref(),
        Some(id.as_str())
    );
    assert_eq!(header(&fetched, "content-disposition"), None);
    let body = axum::body::to_bytes(fetched.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body, "deb");

    // The same through the router, which fills the handler's path from the url's five parts.
    let router = Router::new()
        .route(
            "/api/pipeline/{name}/run/{run}/artifact/{stage}/{task}/{artifact}",
            get(artifacts::get_pipeline_artifact),
        )
        .with_state(boss.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    spawn(async move { axum::serve(listener, router).await });
    let url =
        format!("http://{address}/api/pipeline/{name}/run/latest/artifact/Stage0/task_0_0/app.deb");
    let served = reqwest::get(&url).await.expect("get");
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(
        served.headers()["bettertest-run"].to_str().unwrap(),
        id.as_str()
    );
    assert_eq!(served.text().await.unwrap(), "deb");

    std::fs::remove_dir_all(&dir).expect("clean up");
    std::fs::remove_file(&file).expect("remove pipedef");
}

/// Asked for an artifact a task did not publish, the boss says what it did publish.
#[tokio::test]
async fn a_missing_artifact_lists_what_the_task_published() {
    let pipeline = pipedef::parse(&big_pipedef(1, 1)).await.expect("parses");
    let boss = boss(BTreeMap::new());
    let run = (boss.db())
        .insert_run("app", &pipeline, None, &BTreeMap::new())
        .expect("run");
    let task = run.stages[0].tasks[0].id;
    boss.db()
        .set_task_state(task, TaskState::Pass, 1)
        .expect("pass");
    let missing = || {
        let found = boss.find_artifact(&run, true, "Stage0", "task_0_0", "app.rpm");
        found.expect_err("not published")
    };
    assert_eq!(
        missing(),
        "Stage0.task_0_0 published no artifact named 'app.rpm'. It published: nothing."
    );
    for name in ["app.deb", "app.tar"] {
        let artifact = Artifact {
            name: name.into(),
            size: 1,
        };
        boss.db().add_artifact(task, &artifact).expect("artifact");
    }
    assert_eq!(
        missing(),
        "Stage0.task_0_0 published no artifact named 'app.rpm'. It published: app.deb, app.tar."
    );
}

/// A pipedef of `stages` stages of `tasks` tasks each.
fn big_pipedef(stages: usize, tasks: usize) -> String {
    let mut source = String::from(
        "from bettertest import Labels, Stage, task\n\n\nclass Label(Labels):\n    x86 = \"x86\"\n",
    );
    for s in 0..stages {
        source.push_str(&format!(
            "\n\nclass Stage{s}(Stage):\n    labels = (Label.x86,)\n"
        ));
        for t in 0..tasks {
            source.push_str(&format!(
                "\n    @task()\n    def task_{s}_{t}():\n        pass\n"
            ));
        }
    }
    source
}

/// What a browser waits for before it can draw a pipeline page: its stream's first message, the
/// snapshot. Times `boss` answering for pipeline `name`, as a browser asks (taking gzip), up to
/// that message, and returns it with how many bytes it took on the wire.
async fn first_snapshot(boss: &Shared, name: &str) -> (Duration, usize, PipelineSnapshot) {
    use std::io::Write;
    use tokio_stream::StreamExt;
    let started = std::time::Instant::now();
    let headers = HeaderMap::from_iter([(
        header::ACCEPT_ENCODING,
        "gzip, deflate, br".parse().unwrap(),
    )]);
    let response = runs::pipeline_events(State(boss.clone()), headers, AxumPath(name.to_string()))
        .await
        .expect("events");
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    let mut body = response.into_body().into_data_stream();
    let (mut wire, mut gunzip) = (0, flate2::write::GzDecoder::new(vec![]));
    while !gunzip.get_ref().ends_with(b"\n\n") {
        let bytes = body.next().await.expect("a message").expect("bytes");
        wire += bytes.len();
        gunzip.write_all(&bytes).expect("gzip");
        gunzip.flush().expect("gzip");
    }
    let took = started.elapsed();
    let text = String::from_utf8(gunzip.get_ref().clone()).expect("utf-8");
    let data = text.trim().strip_prefix("data: ").expect("data");
    match serde_json::from_str(data).expect("json") {
        Live::Snapshot(snapshot) => (took, wire, snapshot),
        Live::Event(event) => panic!("expected a snapshot first, got {event:?}"),
    }
}

/// A live stream is gzipped only for a browser that takes gzip, and says so either way it varies.
#[tokio::test]
async fn a_live_stream_is_gzipped_only_when_the_browser_takes_gzip() {
    let boss = boss(BTreeMap::new());
    for (accept, encoding) in [(Some("gzip, br"), Some("gzip")), (None, None)] {
        let mut headers = HeaderMap::new();
        if let Some(accept) = accept {
            headers.insert(header::ACCEPT_ENCODING, accept.parse().unwrap());
        }
        let response = workers::workers_events(State(boss.clone()), headers)
            .await
            .expect("events");
        let got = |name| response.headers().get(name).map(|v| v.to_str().unwrap());
        assert_eq!(got(header::CONTENT_TYPE), Some("text/event-stream"));
        assert_eq!(got(header::CACHE_CONTROL), Some("no-cache"));
        assert_eq!(got(header::VARY), Some("accept-encoding"));
        assert_eq!(got(header::CONTENT_ENCODING), encoding);
    }
}

/// A page load of a big pipeline (10 stages of 20 tasks) with a long history (300 runs) answers
/// within 100 ms, in under 400 KB (its JSON is 1.4 MB): only the newest RECENT_RUNS runs come out of the db, in three
/// queries; the pipedef, parsed once already as the boss does at startup, is not parsed again; and
/// the 10 000 task runs of the snapshot go gzipped.
#[tokio::test]
async fn a_big_pipeline_with_a_long_history_loads_at_once() {
    let source = big_pipedef(10, 20);
    let file = std::env::temp_dir().join(format!("big-{}.py", Uuid::new_v4()));
    std::fs::write(&file, &source).expect("write pipedef");
    let boss = boss(BTreeMap::new());
    lock(&boss.pipedefs).insert("big".into(), Pipedef::File(file.clone()));
    // What the boss does at startup.
    let (_, pipeline) = runs::load_pipeline(&boss, "big", None).await.expect("load");
    let pipeline = pipeline.expect("parses");
    let mut newest = vec![];
    for i in 0..300 {
        let mut db = boss.db();
        let run = db
            .insert_run("big", &pipeline, None, &BTreeMap::new())
            .expect("run");
        for task in run.stages.iter().flat_map(|stage| &stage.tasks) {
            db.start_task(task.id, "http://worker:7358", i)
                .expect("start");
            db.set_task_state(task.id, TaskState::Pass, i + 1)
                .expect("pass");
        }
        db.set_run_state(run.id, RunState::Pass).expect("state");
        newest.insert(0, run.id);
    }
    // The quickest of a few, so a test running alongside cannot make it fail.
    let mut quickest = Duration::MAX;
    for _ in 0..5 {
        let (took, wire, snapshot) = first_snapshot(&boss, "big").await;
        quickest = quickest.min(took);
        assert!(wire < 400_000, "the snapshot took {wire} bytes on the wire");
        let ids: Vec<Uuid> = snapshot.runs.iter().map(|run| run.id).collect();
        assert_eq!(ids, newest[..RECENT_RUNS], "the newest runs, newest first");
        assert!(snapshot.runs.iter().all(|run| run.stages.len() == 10
            && run.stages.iter().all(|stage| stage.tasks.len() == 20
                && stage.tasks.iter().all(|task| task.state == TaskState::Pass))));
    }
    std::fs::remove_file(&file).expect("remove pipedef");
    assert!(
        quickest < Duration::from_millis(100),
        "the snapshot took {quickest:?}"
    );
}

/// The pipeline page loads every pipeline in one stream (a browser opens at most six connections
/// over HTTP/1.1): a snapshot of each, with its pipedef and runs, then the events of every one.
#[tokio::test]
async fn every_pipeline_loads_in_one_stream_with_the_events_of_all() {
    use tokio_stream::StreamExt;
    let boss = boss(BTreeMap::new());
    let mut files = vec![];
    for (name, stages) in [("first", 1), ("second", 2)] {
        let file = std::env::temp_dir().join(format!("{name}-{}.py", Uuid::new_v4()));
        std::fs::write(&file, big_pipedef(stages, 1)).expect("write pipedef");
        lock(&boss.pipedefs).insert(name.into(), Pipedef::File(file.clone()));
        files.push(file);
    }
    let (_, second) = runs::load_pipeline(&boss, "second", None)
        .await
        .expect("load");
    let second = second.expect("parses");
    let start = |boss: &Shared| {
        boss.record(|db| {
            let run = db.insert_run("second", &second, None, &BTreeMap::new())?;
            Ok((Event::RunStarted(run.clone()), run))
        })
        .expect("run")
    };
    let old = start(&boss);

    let response = runs::pipelines_events(State(boss.clone()), HeaderMap::new())
        .await
        .expect("events");
    let mut body = response.into_body().into_data_stream();
    let mut text = String::new();
    let mut next = async || -> Live<Vec<PipelineSnapshot>> {
        while !text.contains("\n\n") {
            let bytes = body.next().await.expect("a message").expect("bytes");
            text.push_str(std::str::from_utf8(&bytes).expect("utf-8"));
        }
        let (message, rest) = text.split_once("\n\n").expect("a message");
        let data = message.strip_prefix("data: ").expect("data");
        let live = serde_json::from_str(data).expect("json");
        text = rest.to_string();
        live
    };

    let Live::Snapshot(snapshots) = next().await else {
        panic!("expected a snapshot first");
    };
    let names: Vec<&str> = snapshots.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["first", "second"]);
    let stages = |s: &PipelineSnapshot| s.pipeline.as_ref().expect("parses").stages.len();
    let runs = |s: &PipelineSnapshot| s.runs.iter().map(|run| run.id).collect::<Vec<_>>();
    let (first_snapshot, second_snapshot) = (&snapshots[0], &snapshots[1]);
    assert_eq!((stages(first_snapshot), runs(first_snapshot)), (1, vec![]));
    assert_eq!(
        (stages(second_snapshot), runs(second_snapshot)),
        (2, vec![old.id])
    );
    assert!(snapshots.iter().all(|s| s.pipelines == ["first", "second"]));

    let new = start(&boss);
    match next().await {
        Live::Event(Event::RunStarted(run)) => {
            assert_eq!((run.pipeline.as_str(), run.id), ("second", new.id))
        }
        other => panic!("expected the second pipeline's new run, got {other:?}"),
    }
    for file in files {
        std::fs::remove_file(file).expect("remove pipedef");
    }
}

/// A pipeline page shows the pipeline from memory, and a pipedef file that changed since, as it is
/// now.
#[tokio::test]
async fn an_edited_pipedef_shows_up_on_its_page() {
    let file = std::env::temp_dir().join(format!("edited-{}.py", Uuid::new_v4()));
    std::fs::write(&file, big_pipedef(1, 1)).expect("write pipedef");
    let boss = boss(BTreeMap::new());
    lock(&boss.pipedefs).insert("edited".into(), Pipedef::File(file.clone()));
    let tasks = |pipeline: Pipeline| pipeline.stages.iter().map(|s| s.tasks.len()).sum::<usize>();
    let shown = runs::shown_pipeline(&boss, "edited").await.expect("shown");
    assert_eq!(tasks(shown.expect("parses")), 1);
    std::fs::write(&file, big_pipedef(2, 3)).expect("edit pipedef");
    let shown = runs::shown_pipeline(&boss, "edited").await.expect("shown");
    assert_eq!(tasks(shown.expect("parses")), 6);
    std::fs::remove_file(&file).expect("remove pipedef");
}

#[test]
fn the_web_ui_is_embedded_when_the_frontend_was_built() {
    // Debug reads the files from the build directory; release carries them. `/` is that page.
    let built = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../frontend/dist/index.html"
    ))
    .exists();
    if built {
        assert!(EmbeddedWebAssets::get("index.html").is_some());
    }
}

#[test]
fn the_install_script_names_the_binary_and_the_label() {
    let x86 = install_script("https://real-live.bettertest.dev", "x86_64");
    assert!(x86.starts_with("#!/bin/sh\n"));
    assert!(x86.contains("https://real-live.bettertest.dev/dl/bettertest-x86_64"));
    assert!(x86.contains("Label.x86"));
    assert!(x86.contains("unified \"$HOME/bettertest/hello.py\""));
    assert!(x86.contains("hello, world"));
    let arm = install_script("http://origin:7357", "aarch64");
    assert!(arm.contains("http://origin:7357/dl/bettertest-aarch64"));
    assert!(arm.contains("Label.arm"));
    assert!(!arm.contains("Label.x86"));
}

#[test]
fn the_install_host_is_only_a_host() {
    assert_eq!(
        public_origin(true, "real-live.bettertest.dev").as_deref(),
        Some("https://real-live.bettertest.dev")
    );
    assert_eq!(
        public_origin(false, "origin:7357").as_deref(),
        Some("http://origin:7357")
    );
    assert_eq!(
        public_origin(false, "[::1]:7357").as_deref(),
        Some("http://[::1]:7357")
    );
    assert!(public_origin(false, "evil.com;rm -rf /").is_none());
    assert!(public_origin(false, "").is_none());
    assert!(public_origin(false, "$(id)").is_none());
}

#[test]
fn install_arch_is_x86_64_or_aarch64() {
    assert_eq!(download_arch(None).unwrap(), "x86_64");
    assert_eq!(download_arch(Some("arch=x86_64")).unwrap(), "x86_64");
    assert_eq!(
        download_arch(Some("foo=1&arch=aarch64")).unwrap(),
        "aarch64"
    );
    assert!(download_arch(Some("arch=arm64")).is_err());
    assert!(download_arch(Some("arch=x86_64&arch=aarch64")).is_err());
    assert!(download_arch(Some("arch=x86_64&arch=x86_64")).is_err());
    assert_eq!(download_arch(Some("")).unwrap(), "x86_64");
    assert_eq!(download_arch(Some("arch&x=1")).unwrap(), "x86_64");
    assert!(download_arch(Some("arch=")).is_err());
    assert!(download_arch(Some("arch=aarch64=x")).is_err());
    assert!(binary_file("../etc/passwd").is_none());
    assert!(
        binary_file("bettertest-x86_64")
            .unwrap()
            .ends_with("bettertest-x86_64")
    );
}
