pub const LIB: &str = include_str!("../../pylib/src/bettertest/__init__.py");
pub const GLUE_SCRIPT: &str = include_str!("../scripts/glue.py");

// Payload to be passed to glue script
// contains bettertest library and pipedef
pub fn stdin_payload(pipedef: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "bettertest": LIB,
        "pipedef": pipedef,
    }))
    .unwrap()
}
