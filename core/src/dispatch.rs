use crate::{client, pipedef};
use bettertest_shared::{Artifact, LogStream, Run};
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tokio::sync::watch;

/// Runs one task straight on its worker, no boss involved, and prints its log to the terminal.
/// The worker cannot tell the difference: it is the same code path the boss uses.
/// `worker` is for a task that picks its worker by labels: dispatch has no workers to pick from.
pub async fn entry(
    path: &Path,
    stage_name: &str,
    task_name: &str,
    worker: Option<String>,
) -> Result<ExitCode, Box<dyn Error>> {
    let source = std::fs::read_to_string(path)?;
    let pipeline = pipedef::parse(&source).await?;
    let Some(stage) = pipeline.stages.iter().find(|s| s.name == stage_name) else {
        return Err(format!("pipedef has no stage {stage_name:?}").into());
    };
    let Some(task) = stage.tasks.iter().find(|t| t.name == task_name) else {
        return Err(format!("stage {stage_name:?} has no task {task_name:?}").into());
    };

    let named = match &stage.sandbox {
        Some(sandbox) => sandbox.worker.clone(),
        None => task.worker.clone(),
    };
    let Some(worker) = worker.or(named) else {
        return Err("this task picks its worker by labels: name one with --worker".into());
    };

    let http = client::http()?;
    let passed = client::execute_task(&http, &worker, &source, stage, task, &mut Terminal).await;
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Prints the pipedef's stages and tasks as JSON. Does not touch a worker.
pub async fn parse(path: &Path) -> Result<ExitCode, Box<dyn Error>> {
    let source = std::fs::read_to_string(path)?;
    let pipeline = pipedef::parse(&source).await?;
    println!("{}", serde_json::to_string_pretty(&pipeline)?);
    Ok(ExitCode::SUCCESS)
}

/// Dispatch runs one task outside any run. Its log goes to the terminal, and the artifacts it
/// publishes to the current directory.
struct Terminal;

impl client::TaskIo for Terminal {
    fn log(&mut self, stream: LogStream, line: String) {
        match stream {
            LogStream::Stdout => println!("{line}"),
            LogStream::Stderr => eprintln!("{line}"),
            LogStream::Bettertest => eprintln!("[bettertest] {line}"),
        }
    }

    fn outputs(&self) -> Result<PathBuf, String> {
        Ok(PathBuf::from("."))
    }

    fn saved(&mut self, _: Artifact) {}

    fn run(&self) -> Option<&Run> {
        None
    }

    fn boss_name(&self) -> Option<&str> {
        None
    }

    // Ctrl-C stops dispatch.
    fn cancelled(&self) -> Option<watch::Receiver<Option<String>>> {
        None
    }

    fn boss_token(&self) -> Option<&str> {
        None
    }
}
