// Tests of pipedef.rs, a child module of it (see the #[path] at its end).

use super::parse;
use std::future::Future;

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
