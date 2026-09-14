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
    Disabled,
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Disabled => "disabled",
        })
    }
}

impl std::str::FromStr for TaskState {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "pass" => Ok(Self::Pass),
            "fail" => Ok(Self::Fail),
            "disabled" => Ok(Self::Disabled),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: Uuid,
    pub name: String,
    pub state: TaskState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageRun {
    pub id: Uuid,
    pub name: String,
    pub tasks: Vec<TaskRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlRun {
    pub id: Uuid,
    pub num: i64,
    pub stages: Vec<StageRun>,
}

#[derive(Serialize, Deserialize)]
pub struct StateResponse {
    #[serde(rename = "pipeline")]
    pub pl: Pipeline,
    pub runs: Vec<PlRun>,
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
