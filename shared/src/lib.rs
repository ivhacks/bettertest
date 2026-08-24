use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    pub worker: String,
    pub image: Option<String>,
    #[serde(default)]
    pub disabled: bool,
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
    pub stages: Vec<StageRun>,
}

#[derive(Serialize, Deserialize)]
pub struct StateResponse {
    pub pipeline: Pipeline,
    pub runs: Vec<Run>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    pub task_id: Uuid,
    pub line: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskLog {
    pub task_id: Uuid,
    pub stage: String,
    pub name: String,
    pub output: String,
}
