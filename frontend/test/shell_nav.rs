// Drives the shell in a browser: navigation, history, and the bytes on the wire.
// `python3 frontend/test/shell.py` is the same test.

#[test]
fn shell_stays_across_navigation() {
    let mut command = std::process::Command::new("python3");
    command.arg("test/shell.py");
    command.current_dir(env!("CARGO_MANIFEST_DIR"));
    let status = command.status().expect("python3 test/shell.py");
    assert!(
        status.success(),
        "playwright shell tests failed (exit {status})"
    );
}
