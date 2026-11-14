// Drives the shell in a browser: navigation, history, and the bytes on the wire. The browser runs
// in Playwright's own image, so beyond trunk, which builds the frontend, this needs nothing from
// the machine but docker. By hand, from frontend/, once this test has built the image:
//   python3 test/shell.py build
//   docker run --rm --init -v "$PWD:/frontend:ro,z" -w /frontend bettertest-test-playwright \
//     python3 test/shell.py

use std::io::Write;
use std::process::{Command, Stdio};

/// Playwright's image holds its browsers but not its Python package; both at one version.
const PLAYWRIGHT: &str = "\
FROM mcr.microsoft.com/playwright/python:v1.65.1-noble
RUN pip install --break-system-packages --no-cache-dir playwright==1.65.1
";

#[test]
fn shell_stays_across_navigation() {
    let root = env!("CARGO_MANIFEST_DIR");
    let built = Command::new("python3")
        .args(["test/shell.py", "build"])
        .current_dir(root)
        .status()
        .expect("python3 test/shell.py build");
    assert!(
        built.success(),
        "could not build the frontend (exit {built})"
    );
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
        .expect("docker build");
    build
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(PLAYWRIGHT.as_bytes())
        .expect("write Dockerfile");
    let built = build.wait().expect("docker build");
    assert!(
        built.success(),
        "could not build the playwright image (exit {built})"
    );
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
        .args(["bettertest-test-playwright", "python3", "test/shell.py"])
        .status()
        .expect("docker run");
    assert!(
        status.success(),
        "playwright shell tests failed (exit {status})"
    );
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
