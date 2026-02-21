use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StageDto {
    pub name: String,
    pub tasks: Vec<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PipelineDto {
    pub stages: Vec<StageDto>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema)]
pub enum TaskState {
    Pending,
    Running,
    Pass,
    Fail,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskRunState {
    pub name: String,
    pub state: TaskState,
    #[serde(default)]
    pub output: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StageRunState {
    pub name: String,
    pub tasks: Vec<TaskRunState>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PipelineRunState {
    pub run_id: u32,
    pub active: bool,
    pub stages: Vec<StageRunState>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct StateResponse {
    pub pipeline: PipelineDto,
    pub run: Option<PipelineRunState>,
}
