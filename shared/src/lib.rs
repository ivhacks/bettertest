use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    pub worker: String,
    pub image: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pipeline {
    pub stages: Vec<Stage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Pass,
    Fail,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: Uuid,
    pub name: String,
    pub state: TaskState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageRun {
    pub name: String,
    pub tasks: Vec<TaskRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: Uuid,
    pub number: i64,
    pub active: bool,
    pub stages: Vec<StageRun>,
}

#[derive(Serialize, Deserialize)]
pub struct RunResponse {
    pub id: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct StateResponse {
    pub pipeline: Pipeline,
    pub runs: Vec<Run>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskUpdate {
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub state: TaskState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunDone {
    pub run_id: Uuid,
}
