use bettertest_shared_crate::*;
use serde_json::*;
use std::{path::*, process::*};

const LIB: &str = include_str!("../../pylib/src/bettertest/__init__.py");

pub fn parse(path: &Path) -> Pipeline {
    let output = Command::new("python3")
        .arg("-c")
        .arg(LIB)
        .arg(path)
        .output()
        .expect("failed to run python3 — is it installed?");

    if !output.status.success() {
        panic!(
            "pipedef dump failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout).expect("python output wasn't utf8");
    from_str(&stdout).unwrap_or_else(|e| panic!("failed to parse pipedef json: {e}\nraw: {stdout}"))
}
