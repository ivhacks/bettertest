use bettertest_shared::Pipeline;
use std::process::Stdio;
use tokio::{io::AsyncWriteExt, process::Command};

// The python half of bettertest travels inside the binary. Nothing is installed on any machine.
pub const LIBRARY: &str = include_str!("../../pylib/src/bettertest/__init__.py");
pub const GLUE: &str = include_str!("../scripts/glue.py");

/// What glue.py reads from stdin. `target` tells run() where to send commands, `files` where the
/// task's artifacts go and come from (both null while parsing).
pub fn glue_stdin(pipedef: &str, target: serde_json::Value, files: serde_json::Value) -> Vec<u8> {
    serde_json::json!({
        "bettertest": LIBRARY,
        "pipedef": pipedef,
        "target": target,
        "files": files,
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
    stdin
        .write_all(&glue_stdin(
            pipedef,
            serde_json::Value::Null,
            serde_json::Value::Null,
        ))
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
