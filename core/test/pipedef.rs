// Tests of pipedef.rs, a child module of it (see the #[path] at its end).

use super::parse;
use bettertest_shared::Task;
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
    assert_eq!(pipeline.sandboxes["DEBIAN"].labels, ["x86".to_string()]);
    let json = serde_json::to_string(&pipeline).expect("json");
    assert!(
        !json.contains("\"worker\""),
        "the pipedef json still has a worker field: {json}"
    );
    // A worker url left in old json is not a field the boss can schedule on.
    let with_url = r#"{"name":"world","worker":"http://pinned:7358","labels":["x86"],"image":null,"vm":null,"arch":null,"timeout":null,"sandbox":null}"#;
    let task: Task = serde_json::from_str(with_url).expect("task json");
    assert_eq!(task.labels, ["x86"]);
    assert!(
        !serde_json::to_string(&task)
            .expect("json")
            .contains("pinned")
    );
}

#[test]
fn a_pipedef_cannot_schedule_by_worker_url() {
    let cases = [
        r#"
from bettertest import Stage, task

class Hello(Stage):
    @task(worker="http://worker:7358")
    def world():
        pass
"#,
        r#"
from bettertest import Stage, task

class Hello(Stage):
    @task("http://worker:7358")
    def world():
        pass
"#,
        r#"
from bettertest import Labels, Sandbox, Stage, task

class Label(Labels):
    x86 = "x86"

BOX = Sandbox(worker="http://worker:7358", image="debian")

class Hello(Stage):
    @task(sandbox=BOX)
    def world():
        pass
"#,
        r#"
from bettertest import Labels, Stage, task

class Label(Labels):
    x86 = "x86"

class Hello(Stage):
    worker = "http://worker:7358"
    labels = [Label.x86]

    @task()
    def world():
        pass
"#,
    ];
    for source in cases {
        let err = run(parse(source)).expect_err("a worker url should not parse");
        assert!(
            err.contains("cannot name a worker url"),
            "rejected for the wrong reason:\n{err}"
        );
    }
}

#[test]
fn a_task_with_no_labels_does_not_parse() {
    let source = r#"
from bettertest import Stage, task

class Hello(Stage):
    @task()
    def world():
        pass
"#;
    let err = run(parse(source)).expect_err("labels are required");
    assert!(err.contains("needs labels"), "{err}");
}
