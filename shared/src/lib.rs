use serde::{Deserialize, Serialize};

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests_module {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}

#[derive(Serialize, Deserialize)]
pub struct Task {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub tasks: Vec<Task>,
}

#[derive(Serialize, Deserialize)]
pub struct Run {
    pub id: u32,
    pub active: bool,
    pub stages: Vec<Stage>,
}

#[derive(Serialize, Deserialize)]
pub struct PipelineResponse {
    pub name: String,
    pub stage_headers: Vec<String>,
    pub runs: Vec<Run>,
    pub pipelines: Vec<String>,
}
