use bettertest_shared::Pipeline;
use serde_json::Value;
use std::process::Stdio;
use tokio::{io::AsyncWriteExt, process::Command};

// The python half of bettertest travels inside the binary. Nothing is installed on any machine.
pub const LIBRARY: &str = include_str!("../../pylib/src/bettertest/__init__.py");
pub const GLUE: &str = include_str!("../scripts/glue.py");

/// The first line glue.py reads from stdin.
pub fn library_line() -> Vec<u8> {
    let mut line = serde_json::Value::from(LIBRARY).to_string().into_bytes();
    line.push(b'\n');
    line
}

/// What glue.py reads from stdin after the library. `target` tells run() where to send commands,
/// `files` where the task's artifacts go and come from (both null while parsing). `task` is the
/// stage and task to start, in the directory `cwd`.
pub fn glue_stdin(
    pipedef: &str,
    target: Value,
    files: Value,
    task: Option<(&str, &str)>,
    cwd: Option<&std::path::Path>,
) -> Vec<u8> {
    serde_json::json!({
        "pipedef": pipedef,
        "target": target,
        "files": files,
        "task": task,
        "cwd": cwd,
    })
    .to_string()
    .into_bytes()
}

/// Runs the pipedef in python and reads back its stages and tasks. Err is python's complaint.
pub async fn parse(pipedef: &str) -> Result<Pipeline, String> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(GLUE)
        .arg("parse")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start python3: {e}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err("python3 has no stdin".into());
    };
    let mut input = library_line();
    input.extend(glue_stdin(pipedef, Value::Null, Value::Null, None, None));
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

pub async fn call_view(pipedef: &str, view: &str, results_json: &str) -> Result<String, String> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(GLUE)
        .arg("view")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start python3: {e}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err("python3 has no stdin".into());
    };
    let results: Value = serde_json::from_str(results_json)
        .map_err(|e| format!("invalid results json: {e}"))?;
    let mut input = library_line();
    input.extend(
        serde_json::json!({
            "pipedef": pipedef,
            "target": Value::Null,
            "files": Value::Null,
            "task": Value::Null,
            "cwd": Value::Null,
            "view": view,
            "results": results,
        })
        .to_string()
        .into_bytes(),
    );
    stdin
        .write_all(&input)
        .await
        .map_err(|e| format!("could not send to python3: {e}"))?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("python3 failed: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "../test/pipedef.rs"]
mod tests;
