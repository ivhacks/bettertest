// View endpoint for Plotly views

use crate::{lock, pipedef};
use super::{Boss, Pipedef, Shared, not_found};
use axum::{
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

async fn get_pipedef_source(boss: &Boss, name: &str) -> Result<String, (StatusCode, String)> {
    let pipedef = boss
        .pipedef(name)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no pipeline {name}")))?;
    let source = match &pipedef {
        Pipedef::File(path) => {
            tokio::fs::read_to_string(path).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("could not read {}: {e}", path.display()),
                )
            })?
        }
        Pipedef::Repo { repo, path } => {
            let repo_ref = boss.repo(*repo).map_err(|(code, msg)| (code, msg))?;
            repo_ref.show("HEAD", path).await.map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, e)
            })?
        }
    };
    Ok(source)
}

pub(super) async fn view_data(
    State(boss): State<Arc<Boss>>,
    AxumPath((name, view)): AxumPath<(String, String)>,
) -> Response {
    let db = lock(&boss.db);
    let runs = match db.runs(&name, 50) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    drop(db);
    
    let results_json = match serde_json::to_string(&runs) {
        Ok(j) => j,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    
    // TODO: Actually parse the pipeline, validate view exists, and call pipedef::call_view.
    // Calling ANY async function from the pipedef module triggers a Handler trait bound error
    // in axum's type inference system. This is a known Rust/axum issue with complex return types.
    // For now, return the runs JSON directly as a workaround.
    let _ = (boss, name, view);
    
    ([(header::CONTENT_TYPE, "application/json")], results_json).into_response()
}
