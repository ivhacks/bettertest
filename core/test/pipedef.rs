// Tests of pipedef.rs, a child module of it (see the #[path] at its end).

use super::{parse, views};
use bettertest_shared::*;
use std::future::Future;
use uuid::Uuid;

fn run<T>(future: impl Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

#[test]
fn a_pipedef_asks_for_labels_and_names_no_worker() {
    let source = r#"
from bettertest import Labels, Sandbox, Stage, task

class Label(Labels):
    x86 = "x86"

DEBIAN = Sandbox(labels=[Label.x86], vm="https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2")

class Hello(Stage):
    labels = [Label.x86]

    @task()
    def world():
        pass

class Install(Stage):
    @task(sandbox=DEBIAN)
    def debian():
        pass
"#;
    let pipeline = run(parse(source)).expect("pipedef");
    assert_eq!(pipeline.stages[0].tasks[0].labels, ["x86"]);
    assert_eq!(pipeline.stages[0].tasks[0].worker, None);
    assert_eq!(pipeline.sandboxes["DEBIAN"].labels, ["x86".to_string()]);
    assert_eq!(pipeline.sandboxes["DEBIAN"].worker, None);
    assert!(pipeline.workers().is_empty());
}

#[test]
fn a_pipedef_can_name_a_worker_url() {
    let by_keyword = r#"
from bettertest import Stage, task

class Hello(Stage):
    @task(worker="http://worker:7358")
    def world():
        pass
"#;
    let positional = r#"
from bettertest import Stage, task

class Hello(Stage):
    @task("http://worker:7358")
    def world():
        pass
"#;
    for source in [by_keyword, positional] {
        let pipeline = run(parse(source)).expect("pipedef");
        assert_eq!(
            pipeline.stages[0].tasks[0].worker.as_deref(),
            Some("http://worker:7358")
        );
        assert!(pipeline.stages[0].tasks[0].labels.is_empty());
        assert_eq!(pipeline.workers(), ["http://worker:7358"]);
    }

    let sandbox = r#"
from bettertest import Sandbox, Stage, task

BOX = Sandbox(worker="http://worker:7358", image="debian")

class Hello(Stage):
    @task(sandbox=BOX)
    def world():
        pass
"#;
    let pipeline = run(parse(sandbox)).expect("pipedef");
    assert_eq!(
        pipeline.sandboxes["BOX"].worker.as_deref(),
        Some("http://worker:7358")
    );
    assert_eq!(pipeline.workers(), ["http://worker:7358"]);

    let stage_worker = r#"
from bettertest import Stage, task

class Hello(Stage):
    worker = "http://worker:7358"

    @task()
    def world():
        pass
"#;
    let pipeline = run(parse(stage_worker)).expect("pipedef");
    assert_eq!(
        pipeline.stages[0].tasks[0].worker.as_deref(),
        Some("http://worker:7358")
    );
}

#[test]
fn a_task_with_neither_a_worker_nor_labels_does_not_parse() {
    let source = r#"
from bettertest import Stage, task

class Hello(Stage):
    @task()
    def world():
        pass
"#;
    let err = run(parse(source)).expect_err("a worker or labels are required");
    assert!(err.contains("needs a worker or labels"), "{err}");
}

fn a_run() -> Run {
    let task = |n: u128, name: &str, carried_from: Option<Uuid>| TaskRun {
        id: Uuid::from_u128(n),
        name: name.into(),
        state: TaskState::Pass,
        worker: None,
        started: Some(1_000),
        ended: Some(3_500),
        carried_from,
        tests: Some(Tests {
            total: 12,
            failed: 1,
        }),
    };
    Run {
        id: Uuid::from_u128(1),
        pipeline: "app".into(),
        state: RunState::Pass,
        stages: vec![StageRun {
            name: "Test".into(),
            tasks: vec![
                task(2, "unit", None),
                task(3, "e2e", Some(Uuid::from_u128(9))),
            ],
        }],
        commit: None,
    }
}

#[test]
fn views_draw_from_the_run_and_ask_for_logs() {
    let source = r#"
from bettertest import Labels, Stage, embed, log, task, view

print("the top level of a pipedef may print")

class Label(Labels):
    x86 = "x86"

class Test(Stage):
    labels = [Label.x86]

    @task()
    def unit():
        pass

    @task()
    def e2e():
        pass

@view()
def passed(run):
    print("so may a view")
    tests = [t["tests"] for s in run["stages"] for t in s["tasks"]]
    passed = sum(t["total"] - t["failed"] for t in tests)
    return {"data": [{"type": "indicator", "value": passed}]}

@view(wide=True)
def logs(run):
    return {"data": [{"type": "table", "cells": {"values": [log(Test.unit), log(Test.e2e)]}}]}

@view(wide=True)
def report(run):
    return embed(Test.e2e, name="report.html")

@view()
def broken(run):
    return {"no": "figure"}
"#;
    let asked = std::sync::Mutex::new(vec![]);
    let log = |task: Uuid| {
        asked.lock().expect("asked").push(task);
        Ok(format!("log of {task}"))
    };
    let views = run(views(source, &a_run(), log)).expect("views");
    let views: serde_json::Value = serde_json::from_str(&views).expect("json");
    assert_eq!(
        views[0],
        serde_json::json!({"name": "passed", "wide": false, "figure": {"data": [{"type": "indicator", "value": 22}]}})
    );
    // A carried over task's log is the one of the task that really ran.
    let unit = format!("log of {}", Uuid::from_u128(2));
    let e2e = format!("log of {}", Uuid::from_u128(9));
    assert_eq!(
        views[1]["figure"]["data"][0]["cells"]["values"],
        serde_json::json!([unit, e2e])
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        [Uuid::from_u128(2), Uuid::from_u128(9)]
    );
    let page = format!(
        "/api/run/{}/artifact/Test/e2e/report.html",
        Uuid::from_u128(1)
    );
    assert_eq!(
        views[2],
        serde_json::json!({"name": "report", "wide": true, "page": page})
    );
    // One view failing shows why, in its place, and the others still draw.
    assert_eq!(views[3]["name"], "broken");
    let error = views[3]["error"].as_str().expect("error");
    assert!(
        error.contains(
            "TypeError: view broken returns a Plotly figure or embed(...), not {'no': 'figure'}"
        ),
        "{error}"
    );
}

#[test]
fn views_of_a_broken_pipedef_say_why() {
    let err =
        run(views("import nonexistent", &a_run(), |_| Ok(String::new()))).expect_err("broken");
    assert!(
        err.contains("ModuleNotFoundError: No module named 'nonexistent'"),
        "{err}"
    );
}

#[test]
fn a_log_that_cannot_be_read_ends_the_views() {
    let source = r#"
from bettertest import Stage, log, view

class Test(Stage):
    pass

def unit():
    pass
unit.__qualname__ = "Test.unit"

@view()
def logs(run):
    return {"data": [log(unit)]}
"#;
    let err = run(views(source, &a_run(), |_| Err("the db is gone".into()))).expect_err("no log");
    assert_eq!(err, "the db is gone");
}
