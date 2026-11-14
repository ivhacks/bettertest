// The pipeline page in a browser: nothing scrolls sideways at any width, and the run grid's cells
// answer the pointer like buttons. `python3 frontend/test/pipeline_page.py` is the same test.

#[test]
fn pipeline_page_fits_and_its_cells_answer() {
    let mut command = std::process::Command::new("python3");
    command.arg("test/pipeline_page.py");
    command.current_dir(env!("CARGO_MANIFEST_DIR"));
    let status = command.status().expect("python3 test/pipeline_page.py");
    assert!(
        status.success(),
        "playwright pipeline page tests failed (exit {status})"
    );
}
