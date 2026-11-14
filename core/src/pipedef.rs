use bettertest_shared::{Pipeline, Run};
use serde::Deserialize;
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};
use uuid::Uuid;

// The python half of bettertest travels inside the binary. Nothing is installed on any machine.
pub const LIBRARY: &str = include_str!("../../pylib/src/bettertest/__init__.py");
pub const GLUE: &str = include_str!("../scripts/glue.py");

/// The first line glue.py reads from stdin.
pub fn library_line() -> Vec<u8> {
    let mut line = serde_json::Value::from(LIBRARY).to_string().into_bytes();
    line.push(b'\n');
    line
}

/// What glue.py reads from stdin after the library, on one line. `target` tells run() where to
/// send commands, `files` where the task's artifacts go and come from (both null while parsing).
/// `task` is the stage and task to start, in the directory `cwd`. `run` is the one views draw.
pub fn glue_stdin(
    pipedef: &str,
    target: Value,
    files: Value,
    task: Option<(&str, &str)>,
    cwd: Option<&std::path::Path>,
    run: Option<&Run>,
) -> Vec<u8> {
    let mut line = serde_json::json!({
        "pipedef": pipedef,
        "target": target,
        "files": files,
        "task": task,
        "cwd": cwd,
        "run": run,
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    line
}

/// glue.py, started to do `command` with the pipedef, its pipes ready to talk to.
fn glue(command: &str) -> Result<Child, String> {
    Command::new("python3")
        .arg("-c")
        .arg(GLUE)
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start python3: {e}"))
}

/// Runs the pipedef in python and reads back its stages and tasks. Err is python's complaint.
pub async fn parse(pipedef: &str) -> Result<Pipeline, String> {
    let mut child = glue("parse")?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err("python3 has no stdin".into());
    };
    let mut input = library_line();
    input.extend(glue_stdin(
        pipedef,
        Value::Null,
        Value::Null,
        None,
        None,
        None,
    ));
    stdin
        .write_all(&input)
        .await
        .map_err(|e| format!("could not send the pipedef to python3: {e}"))?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("python3 failed: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("pipedef dumped bad json: {e}"))
}

/// How long the views of a run may take to draw, all of them.
const VIEWS_TIME: Duration = Duration::from_secs(60);

/// A view's question to the boss, from log() in the library.
#[derive(Deserialize)]
struct Ask {
    log: Uuid,
}

/// Draws the pipedef's views of `run` in python and returns them as JSON for the results page
/// (views_json in the library). As they draw, views ask for the logs of tasks, which `log` reads.
/// Err is python's complaint.
pub async fn views(
    pipedef: &str,
    run: &Run,
    log: impl Fn(Uuid) -> Result<String, String>,
) -> Result<String, String> {
    let mut child = glue("views")?;
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err("python3 has no pipes".into());
    };
    let mut input = library_line();
    input.extend(glue_stdin(
        pipedef,
        Value::Null,
        Value::Null,
        None,
        None,
        Some(run),
    ));
    // Owning stdin, this closes it once done, which ends a python still waiting for an answer.
    let talk = async move {
        let failed = |e: std::io::Error| format!("python3 failed: {e}");
        stdin.write_all(&input).await.map_err(failed)?;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await.map_err(failed)? {
            let Ok(Ask { log: task }) = serde_json::from_str(&line) else {
                return Ok(Some(line));
            };
            let mut answer = Value::from(log(task)?).to_string();
            answer.push('\n');
            stdin.write_all(answer.as_bytes()).await.map_err(failed)?;
        }
        Ok::<_, String>(None)
    };
    // Read alongside, or a view that prints a lot fills the pipe and waits forever.
    let mut complaint = String::new();
    let both = async { tokio::join!(talk, stderr.read_to_string(&mut complaint)).0 };
    let answer = tokio::time::timeout(VIEWS_TIME, both).await;
    let answer = answer.map_err(|_| format!("the views took over {}s", VIEWS_TIME.as_secs()))?;
    answer?.ok_or(complaint)
}

#[cfg(test)]
#[path = "../test/pipedef.rs"]
mod tests;
