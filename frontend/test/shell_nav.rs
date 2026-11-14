// Drives the frontend in a browser: the shell (navigation, history, the bytes on the wire), and the
// pipeline page (nothing scrolls sideways, grid cells answer like buttons). The browser runs in
// Playwright's own image, so this needs nothing from the machine but docker. It tests the frontend
// that frontend/build.sh built (./build.sh runs it), and fails saying so when that is missing or
// older than the source. By hand, from frontend/, once this test has built the image:
//   docker run --rm --init -v "$PWD:/frontend:ro,z" -w /frontend bettertest-test-playwright \
//     python3 test/shell.py    (or test/pipeline_page.py)

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Playwright's image holds its browsers but not its Python package; both at one version.
const PLAYWRIGHT: &str = "\
FROM mcr.microsoft.com/playwright/python:v1.65.1-noble
RUN pip install --break-system-packages --no-cache-dir playwright==1.65.1
";

#[test]
fn shell_stays_across_navigation() {
    playwright("test/shell.py");
}

#[test]
fn pipeline_page_fits_and_its_cells_answer() {
    playwright("test/pipeline_page.py");
}

/// Runs `script` in Playwright's image against the built frontend.
fn playwright(script: &str) {
    let root = env!("CARGO_MANIFEST_DIR");
    // The tests run in parallel; the first builds the image, the rest wait for it.
    static PREPARED: OnceLock<Result<(), String>> = OnceLock::new();
    if let Err(why) = PREPARED.get_or_init(prepare) {
        panic!("{why}");
    }
    let mount = format!("{root}:/frontend:ro,z");
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--init",
            "--volume",
            &mount,
            "--workdir",
            "/frontend",
        ])
        .args(["bettertest-test-playwright", "python3", script])
        .status()
        .expect("docker run");
    assert!(status.success(), "{script} failed (exit {status})");
}

/// Builds Playwright's image, once for all the tests.
fn prepare() -> Result<(), String> {
    require_docker();
    let mut build = Command::new("docker")
        .args([
            "build",
            "--quiet",
            "--tag",
            "bettertest-test-playwright",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| format!("docker build: {e}"))?;
    build
        .stdin
        .take()
        .ok_or("docker build has no stdin")?
        .write_all(PLAYWRIGHT.as_bytes())
        .map_err(|e| format!("write Dockerfile: {e}"))?;
    let built = build.wait().map_err(|e| format!("docker build: {e}"))?;
    if !built.success() {
        return Err(format!(
            "could not build the playwright image (exit {built})"
        ));
    }
    Ok(())
}

/// Docker is the one thing the tests need from the machine. Without it this fails saying so.
fn require_docker() {
    let info = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output();
    let why = match info {
        Ok(out) if out.status.success() => return,
        Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
        Err(e) => format!("could not run docker: {e}"),
    };
    panic!(
        "this test needs docker, the one thing the tests need from the machine: install it and \
         start its daemon, as the cloud VM's start script does. docker info said: {why}"
    );
}
