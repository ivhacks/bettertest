use crate::embedded_scripts::*;
use bettertest_shared_crate::*;
use serde_json::*;
use std::{io::Write, path::*, process::*};

pub fn parse(path: &Path) -> std::result::Result<Pipeline, String> {
    let pipedef = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read pipedef {}: {e}", path.display()))?;
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(GLUE_SCRIPT)
        .arg("parse")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run python3 — is it installed? {e}"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&stdin_payload(&pipedef))
        .map_err(|e| format!("failed to write pipedef to parser: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for pipedef parse: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "pipedef dump failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout =
        String::from_utf8(output.stdout).map_err(|_| "python output wasn't utf8".to_string())?;
    from_str(&stdout).map_err(|e| format!("failed to parse pipedef json: {e}\nraw: {stdout}"))
}
