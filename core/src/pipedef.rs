use crate::embedded_scripts::*;
use bettertest_shared_crate::*;
use serde_json::*;
use std::{io::Write, path::*, process::*};

pub fn parse(path: &Path) -> Pipeline {
    let pipedef = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read pipedef {}: {e}", path.display()));
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(GLUE_SCRIPT)
        .arg("parse")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run python3 — is it installed?");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&stdin_payload(&pipedef))
        .unwrap();
    let output = child
        .wait_with_output()
        .expect("failed to wait for pipedef parse");

    if !output.status.success() {
        panic!(
            "pipedef dump failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout).expect("python output wasn't utf8");
    from_str(&stdout).unwrap_or_else(|e| panic!("failed to parse pipedef json: {e}\nraw: {stdout}"))
}
