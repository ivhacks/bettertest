use axum_typed_multipart::TryFromMultipart;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, TryFromMultipart)]
pub struct StartTaskRequest {
    #[form_data(field_name = "pipedef")]
    pub pipedef_content: String,
    pub stage_name: String,
    pub task_name: String,
}
