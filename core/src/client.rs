// Talks to workers. The boss and the dispatch command both run tasks through here, so a task does
// exactly the same thing whether it was kicked from the web ui or from a laptop.

use crate::{
    workdir,
    worker::{StartContainer, StartVm, Started},
};
use bettertest_shared::{
    Artifact, LogStream, Run, Sandbox, Stage, Task, duration, valid_artifact_name,
};
use reqwest::{Client, Response, multipart::Form};
use std::{fs, path::Path, path::PathBuf, time::Duration};
use tokio::{io::AsyncWriteExt, sync::watch};
use uuid::Uuid;

pub fn http() -> reqwest::Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
}

/// The sandbox a task's commands go to, as `start-task` names it. The worker hands out the ids.
#[derive(Clone)]
pub enum Target {
    None,
    Container(String),
    Vm(String),
}

/// What a task hands back to whoever started it, and what it asks them for.
pub trait TaskIo {
    fn log(&mut self, stream: LogStream, line: String);

    /// Where the artifacts the task publishes are saved.
    fn outputs(&self) -> Result<PathBuf, String>;

    /// An artifact the task published is saved in `outputs()`.
    fn saved(&mut self, artifact: Artifact);

    /// The run the task belongs to. Its task can fetch artifacts from the boss. None outside any
    /// run.
    fn run(&self) -> Option<&Run>;

    /// The name the boss's HTTPS certificate is for, when it speaks HTTPS. The task reaches the boss
    /// at the address the worker saw it call from, and checks the certificate against this name.
    fn boss_name(&self) -> Option<&str>;

    /// The port the boss listens on. The worker combines it with the IP of the start-task
    /// connection; TCP's source port is ephemeral, so this has to come along in the form.
    fn boss_port(&self) -> u16;

    /// Says why, once the run is cancelled. None when nothing can cancel it.
    fn cancelled(&self) -> Option<watch::Receiver<Option<String>>>;

    /// What the task shows the boss to fetch its checkout and artifacts, when there is a boss.
    fn boss_token(&self) -> Option<&str>;

    /// The boss waits when the worker says it is full. Dispatch does not: a 409 is the answer.
    fn wait_when_busy(&self) -> bool {
        false
    }
}

/// How long the boss waits before asking a worker again after it answered 409. The slot it still
/// holds belongs to a task that has already exited, or to a dispatch, and is free within this.
const BUSY_AGAIN: Duration = Duration::from_millis(200);

enum StartError {
    /// The worker is already running as many tasks as it takes. The task did not start.
    Busy(String),
    Failed(String),
}

impl From<String> for StartError {
    fn from(value: String) -> Self {
        StartError::Failed(value)
    }
}

/// Runs one task on `worker` in a sandbox of its own: start it, run the task body, stop it. The
/// sandbox is the stage's if the stage declares one, else the task's. Returns whether the task
/// passed.
pub async fn execute_task(
    http: &Client,
    worker: &str,
    pipedef: &str,
    stage: &Stage,
    task: &Task,
    io: &mut impl TaskIo,
) -> bool {
    let sandbox = stage.sandbox.clone().unwrap_or_else(|| Sandbox {
        worker: task.worker.clone(),
        labels: task.labels.clone(),
        image: task.image.clone(),
        vm: task.vm.clone(),
        arch: task.arch,
        parallel: false,
    });
    let cancelled = io.cancelled();
    let mut log = |stream, line| io.log(stream, line);
    let target = match start_sandbox(http, worker, &sandbox, &mut log, cancelled).await {
        Ok(target) => target,
        Err(e) => {
            io.log(LogStream::Bettertest, e);
            return false;
        }
    };
    let passed = run_task(http, worker, pipedef, &stage.name, task, &target, io).await;
    // A sandbox that will not stop is leaking worker resources. The task did not pass cleanly.
    match stop_sandbox(http, worker, &target).await {
        Ok(()) => passed,
        Err(e) => {
            io.log(LogStream::Bettertest, e);
            false
        }
    }
}

/// Starts the sandbox on `worker`. Cancelling the run stops the wait at once: a VM can take
/// minutes to boot, and a cancelled run should not. The worker still finishes starting it, so it is
/// stopped as soon as it is up rather than left running.
pub async fn start_sandbox(
    http: &Client,
    worker: &str,
    sandbox: &Sandbox,
    log: &mut impl FnMut(LogStream, String),
    mut cancelled: Option<watch::Receiver<Option<String>>>,
) -> Result<Target, String> {
    let (kind, image) = match (&sandbox.image, &sandbox.vm) {
        (Some(image), _) => ("container", image),
        (None, Some(image)) => ("vm", image),
        (None, None) => return Ok(Target::None),
    };
    log(LogStream::Bettertest, format!("starting {kind} {image}"));
    let mut starting = tokio::spawn(start(http.clone(), worker.to_string(), sandbox.clone()));
    let target = tokio::select! {
        started = &mut starting => started.map_err(|e| e.to_string())??,
        reason = cancellation(&mut cancelled) => {
            let (http, worker) = (http.clone(), worker.to_string());
            tokio::spawn(async move {
                if let Ok(Ok(target)) = starting.await {
                    let _ = stop_sandbox(&http, &worker, &target).await;
                }
            });
            return Err(reason);
        }
    };
    if let Target::Container(id) | Target::Vm(id) = &target {
        log(LogStream::Bettertest, format!("started {kind} {id}"));
    }
    Ok(target)
}

/// Asks the worker for the sandbox and waits until it is up.
async fn start(http: Client, worker: String, sandbox: Sandbox) -> Result<Target, String> {
    if let Some(image) = sandbox.image {
        let url = format!("{worker}/start-container");
        let request = http.post(&url).json(&StartContainer { image });
        let started: Started = json(check(&url, request.send().await).await?, &url).await?;
        return Ok(Target::Container(started.id));
    }
    if let Some(image) = sandbox.vm {
        let url = format!("{worker}/start-vm");
        let request = http.post(&url).json(&StartVm {
            image,
            arch: sandbox.arch,
        });
        let started: Started = json(check(&url, request.send().await).await?, &url).await?;
        return Ok(Target::Vm(started.id));
    }
    Ok(Target::None)
}

pub async fn stop_sandbox(http: &Client, worker: &str, target: &Target) -> Result<(), String> {
    let url = match target {
        Target::None => return Ok(()),
        Target::Container(id) => format!("{worker}/container/{id}"),
        Target::Vm(id) => format!("{worker}/vm/{id}"),
    };
    check(&url, http.delete(&url).send().await).await?;
    Ok(())
}

/// Runs the task body on `worker`, in an already started sandbox. Returns whether it passed.
pub async fn run_task(
    http: &Client,
    worker: &str,
    pipedef: &str,
    stage: &str,
    task: &Task,
    target: &Target,
    io: &mut impl TaskIo,
) -> bool {
    // A timeout too big for a Duration is as good as none.
    let limit = task
        .timeout
        .and_then(|t| Duration::try_from_secs_f64(t).ok());
    let started = async {
        let mut said = false;
        loop {
            match start_task(http, worker, pipedef, stage, task, target, io).await {
                Ok(code) => return Ok(code),
                Err(StartError::Failed(e)) => return Err(e),
                // The boss's own count said this worker had a slot. The worker disagreed: a task
                // that just exited still held its permit, or a dispatch is using one. Wait.
                Err(StartError::Busy(why)) if io.wait_when_busy() => {
                    if !said {
                        io.log(LogStream::Bettertest, format!("waiting: {why}"));
                        said = true;
                    }
                    let mut cancelled = io.cancelled();
                    tokio::select! {
                        () = tokio::time::sleep(BUSY_AGAIN) => {}
                        reason = cancellation(&mut cancelled) => return Err(reason),
                    }
                }
                Err(StartError::Busy(why)) => return Err(why),
            }
        }
    };
    let ended = match limit {
        // Giving up drops the connection to the worker, which kills the task and all it started.
        Some(limit) => tokio::time::timeout(limit, started).await.map_err(|_| {
            let ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
            format!("timed out after {}", duration(ms))
        }),
        None => Ok(started.await),
    };
    match ended.and_then(|exit| exit) {
        Ok(exit_code) => {
            io.log(LogStream::Bettertest, format!("exit code {exit_code}"));
            exit_code == 0
        }
        Err(e) => {
            io.log(LogStream::Bettertest, e);
            false
        }
    }
}

/// Streams the task body's output into the log, saves the artifacts it published, and returns its
/// exit code.
async fn start_task(
    http: &Client,
    worker: &str,
    pipedef: &str,
    stage: &str,
    task: &Task,
    target: &Target,
    io: &mut impl TaskIo,
) -> Result<i32, StartError> {
    let mut form = Form::new()
        .text("pipedef", pipedef.to_string())
        .text("stage", stage.to_string())
        .text("task", task.name.clone());
    match target {
        Target::None => {}
        Target::Container(id) => form = form.text("target", format!("container:{id}")),
        Target::Vm(id) => form = form.text("target", format!("vm:{id}")),
    }
    if let Some(run) = io.run() {
        form = form
            .text("pipeline", run.pipeline.clone())
            .text("run", run.id.to_string());
        if let Some(commit) = &run.commit {
            form = form.text("commit", commit.sha.clone());
        }
        if let Some(token) = io.boss_token() {
            form = form.text("boss_token", token.to_string());
        }
        form = form.text("boss_port", io.boss_port().to_string());
    }
    if let Some(name) = io.boss_name() {
        form = form.text("boss_name", name.to_string());
    }

    let url = format!("{worker}/start-task");
    let sent = http
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| StartError::Failed(format!("could not reach {url}: {}", why(&e))))?;
    if sent.status() == reqwest::StatusCode::CONFLICT {
        let body = sent.text().await.unwrap_or_default();
        return Err(StartError::Busy(format!("{url} said 409 Conflict: {body}")));
    }
    let mut response = check_response(&url, sent)
        .await
        .map_err(StartError::Failed)?;

    let mut events = SseDecoder::default();
    let mut cancelled = io.cancelled();
    loop {
        // Hanging up makes the worker kill the task. The caller still stops its sandbox.
        let chunk = tokio::select! {
            chunk = tokio::time::timeout(QUIET, response.chunk()) => chunk,
            reason = cancellation(&mut cancelled) => return Err(reason.into()),
        };
        let chunk = match chunk {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => break,
            Ok(Err(e)) => {
                return Err(format!("lost connection to {url}: {}", why(&e)).into());
            }
            // Frozen, unplugged or gone off the network: a dead machine never says goodbye.
            Err(_) => {
                return Err(format!(
                    "lost {worker}: it has said nothing for {}, not even a keep-alive",
                    duration(QUIET.as_millis().try_into().unwrap_or(u64::MAX))
                )
                .into());
            }
        };
        for (event, data) in events.feed(&chunk) {
            match event.as_str() {
                "stdout" => io.log(LogStream::Stdout, data),
                "stderr" => io.log(LogStream::Stderr, data),
                "artifact" => {
                    let outputs = io.outputs()?;
                    let artifact = save_artifact(http, worker, &data, &outputs).await?;
                    let size = artifact.size;
                    io.log(
                        LogStream::Bettertest,
                        format!("kept artifact {} ({size} bytes)", artifact.name),
                    );
                    io.saved(artifact);
                }
                "exit" => {
                    return data
                        .parse()
                        .map_err(|_| StartError::Failed(format!("bad exit code {data:?}")));
                }
                _ => {}
            }
        }
    }
    Err(format!("{url} ended without an exit code").into())
}

/// A running task's stream from its worker carries a keep-alive every 15 seconds. Silence for
/// longer means the worker is gone, even when the connection never said so.
pub const QUIET: Duration = Duration::from_secs(45);

/// The reason, once `cancelled` says the run is cancelled. Never, without a `cancelled`.
pub async fn cancellation(cancelled: &mut Option<watch::Receiver<Option<String>>>) -> String {
    if let Some(cancelled) = cancelled
        && let Ok(reason) = cancelled.wait_for(Option::is_some).await
    {
        return reason.clone().unwrap_or_default();
    }
    std::future::pending().await
}

/// Downloads `published` (`<id>/<name>`, from an `artifact` event) from the worker into `dir`.
/// The file appears under its name only once it is complete.
async fn save_artifact(
    http: &Client,
    worker: &str,
    published: &str,
    dir: &Path,
) -> Result<Artifact, String> {
    let name = match published.split_once('/') {
        Some((id, name)) if Uuid::parse_str(id).is_ok() && valid_artifact_name(name) => name,
        _ => return Err(format!("the worker published a bad artifact {published:?}")),
    };
    let url = format!("{worker}/output/{published}");
    let mut response = check(&url, http.get(&url).send().await).await?;

    let failed = |e: std::io::Error| format!("could not save artifact {name}: {e}");
    workdir::shared_dir(dir).map_err(failed)?;
    let path = dir.join(name);
    let partial = dir.join(format!(".{name}.{}", Uuid::new_v4().simple()));
    let file = workdir::new_shared_file(&partial).map_err(failed)?;
    let mut file = tokio::fs::File::from_std(file);
    let mut size = 0;
    let written = async {
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("lost connection to {url}: {}", why(&e)))?
        {
            file.write_all(&chunk).await.map_err(failed)?;
            size += chunk.len() as u64;
        }
        file.flush().await.map_err(failed)?;
        fs::rename(&partial, &path).map_err(failed)
    }
    .await;
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }
    written?;
    Ok(Artifact {
        name: name.to_string(),
        size,
    })
}

/// A reqwest error with its causes: "error sending request" alone does not say that the name did
/// not resolve or the connection was refused.
pub fn why(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(next) = cause {
        text.push_str(&format!(": {next}"));
        cause = next.source();
    }
    text
}

/// Turns a failed request or an error status into a message worth putting in a task log.
pub async fn check(url: &str, sent: reqwest::Result<Response>) -> Result<Response, String> {
    let response = sent.map_err(|e| format!("could not reach {url}: {}", why(&e)))?;
    check_response(url, response).await
}

async fn check_response(url: &str, response: Response) -> Result<Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!("{url} said {status}: {body}"))
}

async fn json<T: serde::de::DeserializeOwned>(response: Response, url: &str) -> Result<T, String> {
    response
        .json()
        .await
        .map_err(|e| format!("bad response from {url}: {e}"))
}

/// Reassembles server-sent events from network chunks. A chunk can end anywhere, even in the middle
/// of a multi-byte character, so bytes wait in `buffer` until a whole event (ending in a blank
/// line) has arrived.
#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    /// Returns every complete `(event, data)` pair received so far.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<(String, String)> {
        self.buffer.extend_from_slice(chunk);
        let mut events = vec![];
        while let Some(end) = self.buffer.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = self.buffer.drain(..end + 2).collect();
            let mut event = String::from("message");
            let mut data: Vec<&str> = vec![];
            let block = String::from_utf8_lossy(&block);
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event = value.trim_start().to_string();
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.strip_prefix(' ').unwrap_or(value));
                }
            }
            if !data.is_empty() {
                events.push((event, data.join("\n")));
            }
        }
        events
    }
}

#[cfg(test)]
#[path = "../test/client.rs"]
mod tests;
