// View endpoint for Plotly views

use crate::{lock, pipedef};
use super::{Boss, Pipedef, Shared, not_found};
use axum::{
    extract::{Path as AxumPath, State},
    http::{StatusCode as HttpStatus, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

async fn view_data_impl(boss: Arc<Boss>, name: String, view: String) -> Result<String, Response> {
    // Use spawn_blocking to work around Handler trait inference issues with pipedef async functions
    tokio::task::spawn_blocking(move || {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(async move {
            // Get runs
            let db = lock(&boss.db);
            let runs = match db.runs(&name, 50) {
                Ok(r) => r,
                Err(e) => return Err((HttpStatus::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
            };
            drop(db);
            
            // Get pipedef source
            let pipedef_enum = match boss.pipedef(&name) {
                Some(Pipedef::File(p)) => Pipedef::File(p.clone()),
                Some(Pipedef::Repo { repo, path }) => Pipedef::Repo { repo, path: path.clone() },
                None => return Err((HttpStatus::NOT_FOUND, format!("no pipeline {name}")).into_response()),
            };
            
            let source = match pipedef_enum {
                Pipedef::File(path) => {
                    match tokio::fs::read_to_string(&path).await {
                        Ok(s) => s,
                        Err(e) => return Err((HttpStatus::INTERNAL_SERVER_ERROR, format!("could not read {}: {e}", path.display())).into_response()),
                    }
                }
                Pipedef::Repo { repo, path } => {
                    let repo_ref = match boss.repo(repo) {
                        Ok(r) => r,
                        Err((code, msg)) => return Err((code, msg).into_response()),
                    };
                    match repo_ref.show("HEAD", &path).await {
                        Ok(s) => s,
                        Err(e) => return Err((HttpStatus::INTERNAL_SERVER_ERROR, e).into_response()),
                    }
                }
            };
            
            // Parse pipeline
            let pipeline = match pipedef::parse(&source).await {
                Ok(p) => p,
                Err(e) => return Err((HttpStatus::UNPROCESSABLE_ENTITY, e).into_response()),
            };
            
            // Check view exists
            if !pipeline.views.contains(&view) {
                return Err(not_found(format!("view {view}")).into_response());
            }
            
            // Serialize runs
            let results_json = match serde_json::to_string(&runs) {
                Ok(j) => j,
                Err(e) => return Err((HttpStatus::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
            };
            
            // Call view function
            let json = match pipedef::call_view(&source, &view, &results_json).await {
                Ok(j) => j,
                Err(e) => return Err((HttpStatus::INTERNAL_SERVER_ERROR, e).into_response()),
            };
            
            Ok(json)
        })
    })
    .await
    .map_err(|e| (HttpStatus::INTERNAL_SERVER_ERROR, format!("blocking task panicked: {e}")).into_response())?
}

pub(super) async fn view_data(
    State(boss): State<Arc<Boss>>,
    AxumPath((name, view)): AxumPath<(String, String)>,
) -> Response {
    match view_data_impl(boss, name, view).await {
        Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Err(response) => response,
    }
}
