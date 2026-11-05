// Artifacts: files uploaded to the boss, and files tasks published.

use super::*;

impl Boss {
    pub(super) fn add_artifact(&self, task: Uuid, artifact: Artifact) {
        let written = self.record(|db| {
            db.add_artifact(task, &artifact)?;
            Ok((Event::Artifact { task, artifact }, ()))
        });
        if let Err(e) = written {
            eprintln!("could not record an artifact of task {task}: {e}");
        }
    }

    /// Where artifact `name` of task `stage.task` in `run` is, or why there is none, in words
    /// for the log of the task that asked for it. `this_run` is whether that task is in `run`
    /// itself, which is when moving the producer earlier would help.
    pub(super) fn find_artifact(
        &self,
        run: &Run,
        this_run: bool,
        stage: &str,
        task: &str,
        name: &str,
    ) -> Result<PathBuf, String> {
        let producer = format!("{stage}.{task}");
        let cant = format!("Can't get artifact {name} of {producer}");
        let in_run = if this_run {
            String::new()
        } else {
            format!(" in run {} of {}", run.short_id(), run.pipeline)
        };
        let Some(id) = run
            .stages
            .iter()
            .find(|s| s.name == stage)
            .and_then(|s| s.tasks.iter().find(|t| t.name == task))
            // A task a rerun carried over published nothing itself: the run it came from did.
            .map(|t| t.carried_from.unwrap_or(t.id))
        else {
            let run_name = if this_run {
                "this run".to_string()
            } else {
                format!("run {} of {}", run.short_id(), run.pipeline)
            };
            return Err(format!("{cant}: {run_name} has no task {producer}."));
        };
        let earlier = if this_run {
            format!(
                " Move {producer} to a stage that runs before this one for the artifact to be \
                 available."
            )
        } else {
            String::new()
        };
        let db = self.db();
        match db.task_state(id).map_err(|e| e.to_string())? {
            None | Some(TaskState::Pending) => {
                return Err(format!(
                    "{cant} because {producer} hasn't run yet{in_run}.{earlier}"
                ));
            }
            Some(TaskState::Running) => {
                return Err(format!(
                    "{cant} because {producer} is still running{in_run}.{earlier}"
                ));
            }
            Some(TaskState::Skipped) => {
                return Err(format!(
                    "{cant} because {producer} was skipped{in_run}, so it published nothing."
                ));
            }
            Some(TaskState::Pass | TaskState::Fail) => {}
        }
        let published = db.artifacts(id).map_err(|e| e.to_string())?;
        let (_, deleted) = db.deleted(id).map_err(|e| e.to_string())?;
        if deleted.is_some() {
            return Err(format!(
                "{cant}: retention deleted the artifacts of {producer}{in_run}. The settings page \
                 says how long they are kept."
            ));
        }
        if !published.iter().any(|a| a.name == name) {
            let names: Vec<&str> = published.iter().map(|a| a.name.as_str()).collect();
            let names = if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            };
            return Err(format!(
                "{producer} published no artifact named '{name}'{in_run}. It published: {names}."
            ));
        }
        Ok(task_artifacts_dir(id)
            .map_err(|e| e.to_string())?
            .join(name))
    }

    /// The run of `pipeline` that `run` names: `latest`, the newest one in which `stage.task`
    /// passed and published artifact `name`, or a run id, whole or its first few hex digits.
    pub(super) fn resolve_run(
        &self,
        pipeline: &str,
        run: &str,
        stage: &str,
        task: &str,
        name: &str,
    ) -> Result<Run, String> {
        let cant = format!("Can't get artifact {name} of {pipeline} {stage}.{task}");
        let db = self.db();
        let known = self.pipedef(pipeline).is_some()
            || !db.runs(pipeline, 1).map_err(|e| e.to_string())?.is_empty();
        if !known {
            return Err(format!("{cant}: there is no pipeline named '{pipeline}'."));
        }
        let id = if run == "latest" {
            let latest = db
                .latest_run_with_artifact(pipeline, stage, task, name)
                .map_err(|e| e.to_string())?;
            latest.ok_or_else(|| {
                format!(
                    "{cant}: no run of {pipeline} has {stage}.{task} passing with an artifact \
                     named '{name}'."
                )
            })?
        } else {
            let prefix = run.replace('-', "").to_ascii_lowercase();
            if prefix.is_empty()
                || prefix.len() > 32
                || !prefix.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Err(format!(
                    "{cant}: '{run}' is not a run id. Use LATEST, or a run's id like 6dfde7."
                ));
            }
            let found = db
                .runs_by_prefix(pipeline, &prefix, 10)
                .map_err(|e| e.to_string())?;
            match found.as_slice() {
                [] => return Err(format!("{cant}: {pipeline} has no run {run}.")),
                [id] => *id,
                several => {
                    let ids: Vec<String> = several
                        .iter()
                        .map(|id| id.simple().to_string().chars().take(12).collect())
                        .collect();
                    return Err(format!(
                        "{cant}: run id {run} could be any of {}. Use more of it.",
                        ids.join(", ")
                    ));
                }
            }
        };
        let run = db.run(id).map_err(|e| e.to_string())?;
        run.ok_or_else(|| format!("{cant}: run {id} vanished."))
    }
}

// Artifacts are files anyone can upload and anything can download: docker images for workers
// without internet, vm disk images, build outputs. An artifact never changes once uploaded, so
// workers can cache them by URL.

pub(super) fn bad_artifact_name() -> Failure {
    (
        StatusCode::BAD_REQUEST,
        "artifact names are letters, digits, '.', '-' and '_', not starting with '.'".into(),
    )
}

pub(super) fn artifact_path(name: &str) -> Result<PathBuf, Failure> {
    if !valid_artifact_name(name) {
        return Err(bad_artifact_name());
    }
    Ok(workdir::dir("artifacts").map_err(internal)?.join(name))
}

pub(super) async fn get_artifact(AxumPath(name): AxumPath<String>) -> Result<Response, Failure> {
    let path = artifact_path(&name)?;
    file_response(&path, &name, None).await
}

// Artifacts tasks published with artifact(). Each task has a directory of its own, so names only
// need to be unique within a task.

pub(super) fn task_artifacts_dir(task: Uuid) -> std::io::Result<PathBuf> {
    Ok(workdir::dir("task-artifacts")?.join(task.to_string()))
}

pub(super) async fn get_task_artifact(
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    if !valid_artifact_name(&name) {
        return Err(bad_artifact_name());
    }
    let path = task_artifacts_dir(id).map_err(internal)?.join(&name);
    // Names are plain ascii without quotes, so they fit in the header as they are.
    let disposition = format!("attachment; filename=\"{name}\"");
    file_response(&path, &name, Some(disposition)).await
}

/// Artifact `name` of task `stage.task` in run `id`, for a later task of that run. 404 says why
/// there is none, in words for that task's log.
pub(super) async fn get_run_artifact(
    State(boss): State<Shared>,
    AxumPath((id, stage, task, name)): AxumPath<(String, String, String, String)>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    if !valid_artifact_name(&name) {
        return Err(bad_artifact_name());
    }
    let run = boss.db().run(id).map_err(internal)?;
    let run = run.ok_or_else(|| not_found(format!("run {id}")))?;
    let path = boss
        .find_artifact(&run, true, &stage, &task, &name)
        .map_err(|reason| (StatusCode::NOT_FOUND, reason))?;
    file_response(&path, &name, None).await
}

/// Artifact `name` of task `stage.task` in a run of `pipeline`: `latest` (the newest run in which
/// the task passed and published it) or a run id, whole or its first few hex digits. The run it
/// came from is in the `bettertest-run` header. 404 says why there is none.
pub(super) async fn get_pipeline_artifact(
    State(boss): State<Shared>,
    AxumPath((pipeline, run, stage, task, name)): AxumPath<(
        String,
        String,
        String,
        String,
        String,
    )>,
) -> Result<Response, Failure> {
    if !valid_artifact_name(&name) {
        return Err(bad_artifact_name());
    }
    let not_there = |reason| (StatusCode::NOT_FOUND, reason);
    let run = boss
        .resolve_run(&pipeline, &run, &stage, &task, &name)
        .map_err(not_there)?;
    let path = boss
        .find_artifact(&run, false, &stage, &task, &name)
        .map_err(not_there)?;
    let mut response = file_response(&path, &name, None).await?;
    if let Ok(value) = header::HeaderValue::from_str(&run.id.to_string()) {
        response.headers_mut().insert("bettertest-run", value);
    }
    Ok(response)
}

/// Streams the file at `path`, artifact `name`. With a `disposition`, browsers save it rather than
/// show it.
pub(super) async fn file_response(
    path: &std::path::Path,
    name: &str,
    disposition: Option<String>,
) -> Result<Response, Failure> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(not_found(format!("artifact {name}")));
        }
        Err(e) => return Err(internal(e)),
    };
    let length = file.metadata().await.map_err(internal)?.len();
    let mut response = (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
        ],
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response();
    if let Some(value) = disposition.and_then(|d| header::HeaderValue::from_str(&d).ok()) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

pub(super) async fn put_artifact(
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Result<StatusCode, Failure> {
    let path = artifact_path(&name)?;
    if path.exists() {
        return Err((
            StatusCode::CONFLICT,
            format!("artifact {name} already exists"),
        ));
    }
    // Upload beside the final name (a leading '.' keeps it out of the artifact namespace), then
    // hard link into place: that fails rather than overwrites if someone else got there first.
    let partial = path.with_file_name(format!(".{name}.{}", Uuid::new_v4().simple()));
    let file = workdir::new_shared_file(&partial).map_err(internal)?;
    let written = write_body(tokio::fs::File::from_std(file), body).await;
    let linked = written.and_then(|()| fs::hard_link(&partial, &path).map_err(|e| e.to_string()));
    let _ = fs::remove_file(&partial);
    match linked {
        Ok(()) => Ok(StatusCode::CREATED),
        Err(e) if path.exists() => Err((
            StatusCode::CONFLICT,
            format!("artifact {name} already exists: {e}"),
        )),
        Err(e) => Err(internal(e)),
    }
}

pub(super) async fn write_body(mut file: tokio::fs::File, body: Body) -> Result<(), String> {
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("upload interrupted: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())
}
