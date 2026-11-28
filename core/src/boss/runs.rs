// Runs: starting them (by hand, by push, as restarts), running their stages and tasks, cancelling
// them, and what the pages ask about runs and tasks.

use super::*;

pub(super) struct Going {
    pub(super) pipeline: String,
    pub(super) commit: Option<Commit>,
    // Set to the reason to cancel the run.
    pub(super) cancel: watch::Sender<Option<String>>,
    // Cancelled because a worker it uses is restarting: once stopped, it resumes (see resume).
    pub(super) interrupted: bool,
    pub(super) spanning: Spanning,
}

/// Says why, once a run is cancelled.
pub(super) type Cancelled = watch::Receiver<Option<String>>;

/// Reads the pipedef fresh every time, so edits show up without restarting the boss, and parses it
/// when it is not what was parsed last time. A pipedef in a repo is read at `commit`, or else from
/// the default branch as of the last fetch. A pipedef file has no history: it is read as it is now.
pub(super) async fn load_pipeline(
    boss: &Boss,
    name: &str,
    commit: Option<&Commit>,
) -> Result<(String, Result<Pipeline, String>), Failure> {
    let source = match load_source(boss, name, commit).await? {
        Ok(source) => source,
        Err(e) => return Ok((String::new(), Err(e))),
    };
    if let Some((parsed, pipeline)) = lock(&boss.parsed).get(name)
        && *parsed == source
    {
        return Ok((source, Ok(pipeline.clone())));
    }
    // One that does not parse is parsed again next time: python's complaint may be passing.
    let pipeline = pipedef::parse(&source).await;
    if let Ok(pipeline) = &pipeline {
        lock(&boss.parsed).insert(name.to_string(), (source.clone(), pipeline.clone()));
    }
    Ok((source, pipeline))
}

/// The pipeline as its page shows it, from memory (see Shown). Only a pipedef file that changed
/// since is read and parsed again.
pub(super) async fn shown_pipeline(
    boss: &Boss,
    name: &str,
) -> Result<Result<Pipeline, String>, Failure> {
    let pipedef = boss
        .pipedef(name)
        .ok_or_else(|| not_found(format!("pipeline {name}")))?;
    let stamp = match &pipedef {
        Pipedef::File(path) => file_stamp(path).await,
        Pipedef::Repo { .. } => None,
    };
    if let Some(shown) = lock(&boss.shown).get(name)
        && shown.stamp == stamp
    {
        return Ok(shown.pipeline.clone());
    }
    refresh_shown(boss, name, stamp).await
}

/// Parses pipeline `name` as its page shows it, and keeps it (see Shown), with the file's `stamp`.
pub(super) async fn refresh_shown(
    boss: &Boss,
    name: &str,
    stamp: Option<(SystemTime, u64)>,
) -> Result<Result<Pipeline, String>, Failure> {
    let (_, pipeline) = load_pipeline(boss, name, None).await?;
    let shown = Shown {
        stamp,
        pipeline: pipeline.clone(),
    };
    lock(&boss.shown).insert(name.to_string(), shown);
    Ok(pipeline)
}

/// When a file was modified, and its size. None if the file system will not say.
async fn file_stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The pipedef's source, read as load_pipeline reads it. Err is why git could not show it.
async fn load_source(
    boss: &Boss,
    name: &str,
    commit: Option<&Commit>,
) -> Result<Result<String, String>, Failure> {
    let pipedef = boss
        .pipedef(name)
        .ok_or_else(|| not_found(format!("pipeline {name}")))?;
    match &pipedef {
        Pipedef::File(path) => match tokio::fs::read_to_string(path).await {
            Ok(source) => Ok(Ok(source)),
            Err(e) => Err(internal(format!("could not read {}: {e}", path.display()))),
        },
        Pipedef::Repo { repo, path } => {
            let at = commit.map_or("HEAD", |commit| commit.sha.as_str());
            Ok(boss.repo(*repo)?.show(at, path).await)
        }
    }
}

pub(super) async fn get_pipelines(State(boss): State<Shared>) -> Json<Vec<String>> {
    Json(boss.pipelines())
}

pub(super) async fn get_pipeline(
    State(boss): State<Shared>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<Pipeline>, Failure> {
    let pipeline = shown_pipeline(&boss, &name).await?;
    pipeline
        .map(Json)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))
}

pub(super) async fn pipeline_events(
    State(boss): State<Shared>,
    headers: HeaderMap,
    AxumPath(name): AxumPath<String>,
) -> Result<Response, Failure> {
    let pipeline = shown_and_enrolled(&boss, &name).await?;
    let pipelines = boss.pipelines();
    let snapshot = move |db: &Db| pipeline_snapshot(db, &name, &pipelines, &pipeline).map(Some);
    let wanted = |snapshot: &PipelineSnapshot, event: &Event| {
        event
            .run()
            .is_some_and(|(pipeline, _)| pipeline == snapshot.name)
    };
    live(boss, &headers, snapshot, wanted)
}

/// Every pipeline's snapshot, then the events of all their runs: what the pipeline page holds so a
/// tab paints from memory. One stream for them all, because over HTTP/1.1 a browser opens at most
/// six connections to the boss, and a stream per pipeline would take every one of them.
pub(super) async fn pipelines_events(
    State(boss): State<Shared>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    let pipelines = boss.pipelines();
    let mut parsed = vec![];
    for name in &pipelines {
        // One the boss cannot read shows why in place of its grid, as a broken pipedef does.
        let pipeline = shown_and_enrolled(&boss, name).await;
        parsed.push((name.clone(), pipeline.unwrap_or_else(|(_, why)| Err(why))));
    }
    let snapshot = move |db: &Db| {
        let snapshots = parsed
            .iter()
            .map(|(name, pipeline)| pipeline_snapshot(db, name, &pipelines, pipeline))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(snapshots))
    };
    live(boss, &headers, snapshot, |_, event| event.run().is_some())
}

/// Pipeline `name` as its page shows it. Showing it enrolls the workers it names.
async fn shown_and_enrolled(
    boss: &Shared,
    name: &str,
) -> Result<Result<Arc<Pipeline>, String>, Failure> {
    let pipeline = shown_pipeline(boss, name).await?;
    if let Ok(pipeline) = &pipeline {
        for url in pipeline.workers() {
            boss.enroll(url);
        }
    }
    Ok(pipeline.map(Arc::new))
}

/// What pipeline `name`'s page starts from: its pipedef as shown, and its newest runs.
fn pipeline_snapshot(
    db: &Db,
    name: &str,
    pipelines: &[String],
    pipeline: &Result<Arc<Pipeline>, String>,
) -> rusqlite::Result<PipelineSnapshot> {
    Ok(PipelineSnapshot {
        name: name.to_string(),
        pipelines: pipelines.to_vec(),
        pipeline: pipeline.clone(),
        runs: db
            .runs(name, RECENT_RUNS as i64)?
            .into_iter()
            .map(Arc::new)
            .collect(),
    })
}

/// Starts a run of `name`. A pipeline in a repo runs at the newest commit of the default branch.
pub(super) async fn start_run(
    State(boss): State<Shared>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Run>, Failure> {
    let commit = match boss.pipedef(&name) {
        Some(Pipedef::Repo { repo, .. }) => {
            boss.fetch(repo).await;
            Some(boss.repo(repo)?.head().await.map_err(internal)?)
        }
        _ => None,
    };
    let loaded = load_pipeline(&boss, &name, commit.as_ref()).await?;
    begin_run(&boss, &headers, &name, loaded, commit, BTreeMap::new()).map(Json)
}

/// Where people see pipeline `name`, as the one who called us reached us.
pub(super) fn link(boss: &Boss, headers: &HeaderMap, name: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let scheme = if boss.tls.is_some() { "https" } else { "http" };
    Some(format!("{scheme}://{host}/pipeline/{name}"))
}

/// Starts a run of pipeline `name`, `loaded` as load_pipeline gives it (422 when it does not parse),
/// at `commit`, carrying over the `carried` tasks of an earlier run. `headers` are those of the
/// request that started it, for the link the forge shows.
pub(super) fn begin_run(
    boss: &Shared,
    headers: &HeaderMap,
    name: &str,
    (source, pipeline): (String, Result<Pipeline, String>),
    commit: Option<Commit>,
    carried: BTreeMap<(String, String), TaskRun>,
) -> Result<Run, Failure> {
    let pipeline = pipeline.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let link = link(boss, headers, name);
    for url in pipeline.workers() {
        boss.enroll(url);
    }
    let run = boss
        .record(|db| {
            let run = db.insert_run(name, &pipeline, commit, &carried)?;
            Ok((Event::RunStarted(run.clone()), run))
        })
        .map_err(internal)?;
    let mut going = lock(&boss.going);
    // Only the newest commit of a branch is worth finishing: cancel runs of older ones.
    if let Some(new) = &run.commit {
        let reason = format!("cancelled: {} replaced it", new.label());
        for other in going.values_mut() {
            let replaced = other.pipeline == run.pipeline
                && other
                    .commit
                    .as_ref()
                    .is_some_and(|old| old.branch == new.branch && old.sha != new.sha);
            if replaced {
                other.interrupted = false;
                other.cancel.send_replace(Some(reason.clone()));
            }
        }
    }
    go(boss, &mut going, run.clone(), pipeline, source, link);
    Ok(run)
}

/// Runs `run`, which the db has, from its first pending task on. It is `going` until it is over.
fn go(
    boss: &Shared,
    going: &mut HashMap<Uuid, Going>,
    run: Run,
    pipeline: Pipeline,
    source: String,
    link: Option<String>,
) {
    let (cancel, cancelled) = watch::channel(None);
    let spanning = Spanning::default();
    let this = Going {
        pipeline: run.pipeline.clone(),
        commit: run.commit.clone(),
        cancel,
        interrupted: false,
        spanning: spanning.clone(),
    };
    going.insert(run.id, this);
    let running = Running {
        boss: boss.clone(),
        run: Arc::new(run),
        source: source.into(),
        cancelled,
        spanning,
    };
    spawn(execute_run(running, pipeline, link));
}

/// Goes on with run `id` after `why` (a restart of the boss, or of a worker it uses) cut it
/// short, keeping its id. It starts again from its first stage that has not passed, or from an
/// earlier one that starts a sandbox the rest need, gone with the restart (see resume_from). The
/// tasks that run again keep their logs, which say why. A run whose pipedef no longer lines up
/// with it cannot go on, and fails. `link` is where people see the pipeline, for the forge, if known.
pub(super) async fn resume(boss: Shared, id: Uuid, why: String, link: Option<String>) {
    let read = || {
        let run = boss.db().run(id);
        run.unwrap_or_else(|e| {
            eprintln!("could not read run {id} to resume it: {e}");
            None
        })
    };
    let Some(run) = read() else { return };
    let loaded = match load_pipeline(&boss, &run.pipeline, run.commit.as_ref()).await {
        Ok((source, Ok(pipeline))) if lines_up(&run, &pipeline) => {
            match resume_from(&run, &pipeline) {
                Some(from) => Ok((source, pipeline, from)),
                None => Err("a task failed".into()),
            }
        }
        Ok((_, Ok(_))) => Err("its pipedef no longer has the stages and tasks it had".into()),
        Ok((_, Err(e))) | Err((_, e)) => Err(e),
    };
    let (source, pipeline, from) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => {
            let why = format!("{why}, and the run cannot go on: {e}");
            return end_run(&boss, &run, why, link).await;
        }
    };
    for stage in run.stages.iter().skip(from) {
        for task in stage.tasks.iter().filter(|t| t.state != TaskState::Pending) {
            rewind(&boss, &run, task, &why);
        }
    }
    let Some(run) = read() else { return };
    go(&boss, &mut lock(&boss.going), run, pipeline, source, link);
}

/// Whether `run` has the stages and tasks `pipeline` has, in the same order.
fn lines_up(run: &Run, pipeline: &Pipeline) -> bool {
    let same = |(ran, defined): (&StageRun, &Stage)| {
        let names = ran.tasks.iter().map(|t| &t.name);
        ran.name == defined.name && names.eq(defined.tasks.iter().map(|t| &t.name))
    };
    run.stages.len() == pipeline.stages.len() && run.stages.iter().zip(&pipeline.stages).all(same)
}

/// The stage an interrupted run goes on from: its first stage that has not passed, or an earlier
/// one that starts a sandbox of the pipeline's it needs (see restart_start). None once a task has
/// failed, or every one has passed: then the run is over.
pub(super) fn resume_from(run: &Run, pipeline: &Pipeline) -> Option<usize> {
    let mut tasks = run.stages.iter().flat_map(|s| &s.tasks);
    if tasks.any(|t| t.state == TaskState::Fail) {
        return None;
    }
    let not_passed = |s: &StageRun| s.tasks.iter().any(|t| t.state != TaskState::Pass);
    let at = run.stages.iter().position(not_passed)?;
    Some(restart_start(pipeline, at, None).0)
}

/// Puts `task` of `run` back to pending, to run again: its artifacts go, its log stays and says
/// `why`.
fn rewind(boss: &Boss, run: &Run, task: &TaskRun, why: &str) {
    // Files first, as retention does: should forgetting them fail, nothing refers to them anyway.
    if let Err(e) = task_artifacts_dir(task.id).and_then(fs::remove_dir_all)
        && e.kind() != ErrorKind::NotFound
    {
        eprintln!("could not delete the artifacts of task {}: {e}", task.id);
    }
    let rewound = boss.record(|db| {
        db.rewind_task(task.id)?;
        let event = Event::TaskState {
            pipeline: run.pipeline.clone(),
            run: run.id,
            task: task.id,
            state: TaskState::Pending,
            at: now(),
        };
        Ok((event, ()))
    });
    if let Err(e) = rewound {
        eprintln!("could not put task {} back to pending: {e}", task.id);
    }
    // After the event, which forgets a carried task's log (another run's) on pages that follow it.
    if task.started.is_some() {
        let line = format!("{why}: running this again");
        boss.log(task.id, LogStream::Bettertest, line);
    }
}

/// Ends `run`, which will not go on. A task the restart cut short fails, its log saying `why`, so
/// a failed run shows where it failed; the tasks still to come are skipped. The run passed if every
/// task did.
async fn end_run(boss: &Shared, run: &Run, why: String, link: Option<String>) {
    let tasks = || run.stages.iter().flat_map(|s| &s.tasks);
    for task in tasks() {
        // Started here, and neither passed nor failed: running, or skipped once a worker restarted.
        let cut_short = matches!(task.state, TaskState::Running | TaskState::Skipped)
            && task.started.is_some()
            && task.carried_from.is_none();
        if cut_short {
            boss.log(task.id, LogStream::Bettertest, why.clone());
            boss.set_task_state(run, task.id, TaskState::Fail);
        } else if task.state == TaskState::Pending {
            boss.set_task_state(run, task.id, TaskState::Skipped);
        }
    }
    let (state, status, said) = match tasks().all(|t| t.state == TaskState::Pass) {
        true => (RunState::Pass, "success", "passed"),
        false => (RunState::Fail, "failure", "failed"),
    };
    boss.set_run_state(run, state);
    boss.report(run, status, said, link.as_deref()).await;
}

#[derive(Deserialize)]
pub(super) struct Push {
    #[serde(rename = "ref")]
    pub(super) git_ref: String,
    pub(super) after: String,
    pub(super) repository: PushedRepo,
}

#[derive(Deserialize)]
pub(super) struct PushedRepo {
    pub(super) full_name: String,
}

/// A forge's push webhook (Forgejo, Gitea): fetches the repo and starts every pipeline the pushed
/// commit has, at that commit. Returns the runs.
pub(super) async fn push_hook(
    State(boss): State<Shared>,
    headers: HeaderMap,
    Json(push): Json<Push>,
) -> Result<Json<Vec<Run>>, Failure> {
    // A tag, or a deleted branch, has nothing to run.
    let Some(branch) = push.git_ref.strip_prefix("refs/heads/") else {
        return Ok(Json(vec![]));
    };
    if push.after.chars().all(|c| c == '0') {
        return Ok(Json(vec![]));
    }
    let index = boss
        .repos
        .iter()
        .position(|r| r.full_name.eq_ignore_ascii_case(&push.repository.full_name))
        .ok_or_else(|| not_found(format!("repo {}", push.repository.full_name)))?;
    boss.fetch(index).await;
    let repo = boss.repo(index)?;

    let mut runs = vec![];
    let found = repo.pipedefs(&push.after).await.map_err(internal)?;
    for (name, path) in found {
        // Only pipelines this repo owns: another source may have claimed the name.
        if !matches!(boss.pipedef(&name), Some(Pipedef::Repo { repo, .. }) if repo == index) {
            continue;
        }
        let commit = Commit {
            sha: push.after.clone(),
            branch: branch.to_string(),
        };
        let source = repo.show(&commit.sha, &path).await.map_err(internal)?;
        let pipeline = pipedef::parse(&source).await;
        if let Err(e) = &pipeline {
            eprintln!("{name} at {}: the pipedef is broken: {e}", commit.label());
            let link = link(&boss, &headers, &name);
            let (http, sha, broken) = (&boss.http, &commit.sha, "the pipedef is broken");
            let reported = repo.report(http, sha, &name, "failure", broken, link.as_deref());
            if let Err(e) = reported.await {
                eprintln!("could not tell {} about it: {e}", repo.public);
            }
            continue;
        }
        let (loaded, commit, carried) = ((source, pipeline), Some(commit), BTreeMap::new());
        runs.push(begin_run(&boss, &headers, &name, loaded, commit, carried)?);
    }
    Ok(Json(runs))
}

/// Cancels a running run, all of it: its running tasks are hung up on (which kills them on their
/// workers), sandboxes still starting are stopped once up, and every task still to come is skipped.
/// The same as when a newer commit replaces a run, just asked for by a person.
pub(super) async fn cancel_run(
    State(boss): State<Shared>,
    AxumPath(run): AxumPath<String>,
    ConnectInfo(peer): ConnectInfo<tls::Peer>,
    headers: HeaderMap,
) -> Result<StatusCode, Failure> {
    let id = Uuid::parse_str(&run).map_err(|_| not_found(format!("run {run}")))?;
    let (me, _) = who(&boss, &headers, Some(&peer))?;
    let reason = match me.logged_in {
        true => format!("cancelled by {}", me.name),
        false => "cancelled from the web ui".to_string(),
    };
    match lock(&boss.going).get_mut(&id) {
        Some(going) => {
            going.interrupted = false;
            going.cancel.send_replace(Some(reason));
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err(conflict(format!("run {run} is not running"))),
    }
}

/// `/api/restart/{run}[/{stage}[/{task}]]`: starts a new run of the pipeline at the commit `run`
/// tested, which runs `stage` (or only its `task`) and every stage after it again, and carries over
/// the results of the rest (see `carry_over`). No stage restarts all of it. Returns the new run.
pub(super) async fn restart(
    State(boss): State<Shared>,
    AxumPath(restart): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Run>, Failure> {
    let mut parts = restart.split('/');
    let earlier = find_run(&boss, parts.next().unwrap_or_default())?;
    let (stage, task) = (parts.next(), parts.next());
    if earlier.state == RunState::Running {
        let short = earlier.short_id();
        let why = format!("run {short} is still running. Restart it once it is over.");
        return Err(conflict(why));
    }
    let name = earlier.pipeline.clone();
    let (source, pipeline) = load_pipeline(&boss, &name, earlier.commit.as_ref()).await?;
    let pipeline = pipeline.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let carried = carry_over(&earlier, &pipeline, stage, task)?;
    let loaded = (source, Ok(pipeline));
    begin_run(&boss, &headers, &name, loaded, earlier.commit, carried).map(Json)
}

/// Where a restart from stage `at` (or of only its `task`) really starts. A sandbox of the
/// pipeline's lives only as long as its run, so whatever restarts needs every one of its sandboxes
/// built up again from the first stage that uses it: restarting from there, whole stages.
pub(super) fn restart_start<'a>(
    pipeline: &Pipeline,
    at: usize,
    task: Option<&'a str>,
) -> (usize, Option<&'a str>) {
    let first_use = |name: &str| {
        let uses = |s: &Stage| s.tasks.iter().any(|t| t.sandbox.as_deref() == Some(name));
        pipeline.stages.iter().position(uses)
    };
    let mut start = at;
    loop {
        // What restarts: from `start` on, but of stage `at` only `task`, while `start` is `at`.
        let mut needed = BTreeSet::new();
        for (index, stage) in pipeline.stages.iter().enumerate().skip(start) {
            let only = task.filter(|_| index == at && start == at);
            needed.extend(sandboxes(stage, only));
        }
        match needed.iter().filter_map(|n| first_use(n)).min() {
            Some(earliest) if earliest < start => start = earliest,
            _ => break,
        }
    }
    // A task that runs in a sandbox builds on the tasks before it there: they restart with it.
    let in_sandbox = pipeline
        .stages
        .get(at)
        .is_some_and(|s| !sandboxes(s, task).is_empty());
    match start == at && !(task.is_some() && in_sandbox) {
        true => (at, task),
        false => (start, None),
    }
}

/// The tasks of `earlier` a restart of `stage` (or of only its `task`) carries over instead of running
/// again, by (stage, task). Every stage before `stage` is carried over, so each of its tasks must
/// have passed: the stages after it ran on that result. So are the other tasks of `stage` when only
/// `task` restarts, unless the stage shares a sandbox, where each task builds on the one before and
/// the whole stage restarts. A task the pipedef has gained since runs, and so does the rest of a
/// shared stage it is in. No `stage` carries nothing over.
pub(super) fn carry_over(
    earlier: &Run,
    pipeline: &Pipeline,
    stage: Option<&str>,
    task: Option<&str>,
) -> Result<BTreeMap<(String, String), TaskRun>, Failure> {
    let mut carried = BTreeMap::new();
    let Some(stage) = stage else {
        return Ok(carried);
    };
    let pipeline_name = &earlier.pipeline;
    let Some(at) = pipeline.stages.iter().position(|s| s.name == stage) else {
        return Err(not_found(format!("stage {stage} in {pipeline_name}")));
    };
    let has =
        |task| (pipeline.stages.get(at)).is_some_and(|s| s.tasks.iter().any(|t| t.name == task));
    if let Some(task) = task
        && !has(task)
    {
        return Err(not_found(format!("task {stage}.{task} in {pipeline_name}")));
    }
    let (at, task) = restart_start(pipeline, at, task);
    let earlier_task = |stage: &str, task: &str| {
        let stage = earlier.stages.iter().find(|s| s.name == stage)?;
        stage.tasks.iter().find(|t| t.name == task)
    };
    for (index, this) in pipeline.stages.iter().enumerate().take(at + 1) {
        let mut carried_here = vec![];
        for t in &this.tasks {
            let Some(was) = earlier_task(&this.name, &t.name) else {
                continue;
            };
            if index == at {
                if task.is_some_and(|task| task != t.name) && was.state.is_over() {
                    carried_here.push(was);
                }
                continue;
            }
            if was.state != TaskState::Pass {
                let what = match was.state {
                    TaskState::Fail => "failed",
                    _ => "did not run",
                };
                let (stage_name, task_name, run) = (&this.name, &t.name, earlier.short_id());
                let why = format!(
                    "{stage_name}.{task_name} {what} in run {run}, so the stages after it have \
                     nothing to build on. Restart {stage_name} instead."
                );
                return Err(conflict(why));
            }
            carried_here.push(was);
        }
        // In a stage whose tasks build on each other in one sandbox, restarting one restarts them all.
        let all = carried_here.len() == this.tasks.len();
        let sequential = this.sandbox.as_ref().is_some_and(|s| !s.parallel);
        if sequential && !all {
            continue;
        }
        for was in carried_here {
            carried.insert((this.name.clone(), was.name.clone()), was.clone());
        }
    }
    Ok(carried)
}

/// Stages run one at a time, in the order the pipedef defines them. Once one fails, the rest are
/// skipped.
pub(super) async fn execute_run(r: Running, pipeline: Pipeline, link: Option<String>) {
    let link = link.as_deref();
    r.boss.report(&r.run, "pending", "running", link).await;
    let mut failed = false;
    // The run was created from this pipeline, so its stages and tasks line up one to one.
    for (index, (stage, stage_run)) in pipeline.stages.iter().zip(&r.run.stages).enumerate() {
        // Tasks a restart carried over, or that ran before the run was resumed, keep their result.
        let pending = |t: &&TaskRun| t.state == TaskState::Pending;
        let fresh = || stage_run.tasks.iter().filter(pending);
        if failed || r.is_cancelled() {
            for task_run in fresh() {
                r.skip(task_run.id);
            }
            continue;
        }
        let passed = match &stage.sandbox {
            _ if fresh().next().is_none() => {
                stage_run.tasks.iter().all(|t| t.state == TaskState::Pass)
            }
            Some(sandbox) => run_shared_stage(&r, stage, sandbox, stage_run).await,
            None => run_parallel_stage(&r, &pipeline, index, stage, stage_run).await,
        };
        failed = !passed;
    }
    // A failed or cancelled run leaves sandboxes up that later stages would have used.
    let left = std::mem::take(&mut *lock(&r.spanning));
    for (_, up) in left {
        if let Err(e) = client::stop_sandbox(&r.boss.http, &up.worker, &up.target).await {
            r.boss.log(up.last, LogStream::Bettertest, e);
        }
    }
    let (boss, run) = (&r.boss, &*r.run);
    let going = lock(&boss.going).remove(&run.id);
    let reason = r.cancelled.borrow().clone();
    if going.is_some_and(|going| going.interrupted) {
        let (why, link) = (reason.unwrap_or_default(), link.map(str::to_string));
        spawn(resume(boss.clone(), run.id, why, link));
        return;
    }
    let (state, status, said) = match &reason {
        Some(reason) => (RunState::Fail, "warning", reason.as_str()),
        None if failed => (RunState::Fail, "failure", "failed"),
        None => (RunState::Pass, "success", "passed"),
    };
    boss.set_run_state(run, state);
    boss.report(run, status, said, link).await;
}

/// What every task of a run needs to run: the boss, the run, the pipedef's source, the run's
/// cancel, and the pipeline's sandboxes that are up. Cloned into each task, cheaply.
#[derive(Clone)]
pub(super) struct Running {
    pub(super) boss: Shared,
    pub(super) run: Arc<Run>,
    pub(super) source: Arc<str>,
    pub(super) cancelled: Cancelled,
    pub(super) spanning: Spanning,
}

impl Running {
    fn is_cancelled(&self) -> bool {
        self.cancelled.borrow().is_some()
    }

    /// Records that task `id` is over: passed or failed. One cut short because its run was cancelled
    /// did not fail: it is skipped, like the tasks that never started.
    fn end(&self, id: Uuid, passed: bool) {
        let state = match passed {
            true => TaskState::Pass,
            false if self.is_cancelled() => TaskState::Skipped,
            false => TaskState::Fail,
        };
        self.boss.set_task_state(&self.run, id, state);
    }

    fn skip(&self, id: Uuid) {
        self.boss.set_task_state(&self.run, id, TaskState::Skipped);
    }

    /// Ends task `id`, which could not run, saying why in its log. Returns that it did not pass.
    fn refuse(&self, id: Uuid, why: String) -> bool {
        self.boss.log(id, LogStream::Bettertest, why);
        self.end(id, false);
        false
    }

    /// A worker for task `id` (see Boss::claim_worker). `named` is a worker url from the pipedef, or
    /// the worker a sandbox of this run is already on.
    async fn claim(
        &self,
        id: Uuid,
        named: Option<&str>,
        labels: &[String],
    ) -> Result<String, String> {
        (self.boss)
            .claim_worker(&self.run, id, named, labels, &self.cancelled)
            .await
    }

    /// Starts `sandbox` on `worker` for task `id`, whose log shows it.
    async fn start_sandbox(
        &self,
        id: Uuid,
        worker: &str,
        sandbox: &Sandbox,
    ) -> Result<client::Target, String> {
        let mut log = |stream, line| self.boss.log(id, stream, line);
        let cancelled = Some(self.cancelled.clone());
        client::start_sandbox(&self.boss.http, worker, sandbox, &mut log, cancelled).await
    }

    /// Stops `target` on `worker`. Returns whether it stopped: one that will not is leaking worker
    /// resources, so task `last`, whose log says why, fails, and its stage with it. In a cancelled
    /// run it is skipped instead, like the rest: a worker that is restarting cannot stop anything,
    /// and takes its sandboxes with it.
    async fn stop_sandbox(
        &self,
        worker: &str,
        target: &client::Target,
        last: Option<Uuid>,
    ) -> bool {
        let Err(e) = client::stop_sandbox(&self.boss.http, worker, target).await else {
            return true;
        };
        if let Some(last) = last {
            self.boss.log(last, LogStream::Bettertest, e);
            self.end(last, false);
        }
        false
    }

    /// Runs task `id`'s body on `worker` in `target`, and records how it went.
    async fn run_task(
        &self,
        id: Uuid,
        worker: &str,
        stage: &str,
        task: &Task,
        target: &client::Target,
    ) -> bool {
        let mut io = BossTask::new(&self.boss, &self.run, id, self.cancelled.clone());
        let http = &self.boss.http;
        client::run_task(http, worker, &self.source, stage, task, target, &mut io).await
    }
}

/// Every task gets a sandbox of its own, and they all run at the same time, but for the tasks of
/// each of the pipeline's sandboxes, which take turns in it (see run_in_sandbox). Those a restart
/// carried over do not run again, but count.
pub(super) async fn run_parallel_stage(
    r: &Running,
    pipeline: &Pipeline,
    index: usize,
    stage: &Stage,
    stage_run: &StageRun,
) -> bool {
    let mut tasks = JoinSet::new();
    // The tasks of each of the pipeline's sandboxes, in the order the stage defines them.
    let mut groups: Vec<(String, Vec<(Task, Uuid)>)> = vec![];
    for (task, task_run) in stage.tasks.iter().zip(&stage_run.tasks) {
        if task_run.carried_from.is_some() {
            continue;
        }
        if let Some(name) = &task.sandbox {
            match groups.iter_mut().find(|(n, _)| n == name) {
                Some((_, group)) => group.push((task.clone(), task_run.id)),
                None => groups.push((name.clone(), vec![(task.clone(), task_run.id)])),
            }
            continue;
        }
        let (r, stage, task, id) = (r.clone(), stage.clone(), task.clone(), task_run.id);
        tasks.spawn(async move {
            let worker = match r.claim(id, task.worker.as_deref(), &task.labels).await {
                Ok(worker) => worker,
                Err(e) => return r.refuse(id, e),
            };
            let mut io = BossTask::new(&r.boss, &r.run, id, r.cancelled.clone());
            let http = &r.boss.http;
            let passed =
                client::execute_task(http, &worker, &r.source, &stage, &task, &mut io).await;
            drop(io);
            r.end(id, passed);
            passed
        });
    }
    let ends_here = last_used_in(pipeline, index);
    for (name, group) in groups {
        let Some(sandbox) = pipeline.sandboxes.get(&name).cloned() else {
            for (_, id) in group {
                r.refuse(id, format!("the pipeline has no sandbox {name}"));
            }
            return false;
        };
        let last = ends_here.contains(&name);
        let (r, stage) = (r.clone(), stage.name.clone());
        tasks.spawn(async move { run_in_sandbox(&r, &stage, &name, &sandbox, group, last).await });
    }
    let carried = stage_run.tasks.iter().filter(|t| t.carried_from.is_some());
    let mut passed = carried.into_iter().all(|t| t.state == TaskState::Pass);
    while let Some(joined) = tasks.join_next().await {
        passed &= matches!(joined, Ok(true));
    }
    passed
}

/// The pipeline's sandboxes that are up, by name: tasks of several stages share them.
pub(super) type Spanning = Arc<Mutex<BTreeMap<String, Up>>>;

/// A sandbox of the pipeline's that is up: where, and the last task that used it, whose log shows
/// it stopping when a failed run has to.
pub(super) struct Up {
    pub(super) worker: String,
    pub(super) target: client::Target,
    pub(super) last: Uuid,
}

/// The pipeline's sandboxes that no stage after stage `index` uses: they stop once it is over.
pub(super) fn last_used_in(pipeline: &Pipeline, index: usize) -> BTreeSet<String> {
    let used = |stage: &Stage| sandboxes(stage, None);
    let mut stages = pipeline.stages.iter().skip(index);
    let here = stages.next().map(used).unwrap_or_default();
    let later: BTreeSet<String> = stages.flat_map(used).collect();
    here.difference(&later).cloned().collect()
}

/// The pipeline's sandboxes the tasks of `stage` run in, or only its task `only`.
fn sandboxes(stage: &Stage, only: Option<&str>) -> BTreeSet<String> {
    let tasks = stage
        .tasks
        .iter()
        .filter(|t| only.is_none_or(|only| t.name == only));
    tasks.filter_map(|t| t.sandbox.clone()).collect()
}

/// The tasks of one stage that run in sandbox `name` of the pipeline, one at a time, in order:
/// each builds on what the ones before it did, in this stage or an earlier one. Starts the sandbox
/// if no stage has yet (that shows in the log of the task that needs it first), and stops it when
/// this is the `last` stage to use it. Once one fails, the rest are skipped.
pub(super) async fn run_in_sandbox(
    r: &Running,
    stage: &str,
    name: &str,
    sandbox: &Sandbox,
    group: Vec<(Task, Uuid)>,
    last: bool,
) -> bool {
    let mut passed = true;
    for (task, id) in group {
        if !passed || r.is_cancelled() {
            r.skip(id);
            continue;
        }
        let up = lock(&r.spanning).remove(name);
        // A sandbox already up stays on the worker that has it. The first task uses the url the
        // pipedef named, or is placed by labels.
        let (named, labels) = match &up {
            Some(up) => (Some(up.worker.as_str()), &[][..]),
            None => (sandbox.worker.as_deref(), &sandbox.labels[..]),
        };
        let worker = match r.claim(id, named, labels).await {
            Ok(worker) => worker,
            Err(e) => {
                passed = r.refuse(id, e);
                if let Some(up) = up {
                    lock(&r.spanning).insert(name.to_string(), up);
                }
                continue;
            }
        };
        let target = match up {
            Some(up) => up.target,
            None => match r.start_sandbox(id, &worker, sandbox).await {
                Ok(target) => target,
                Err(e) => {
                    passed = r.refuse(id, e);
                    continue;
                }
            },
        };
        passed = r.run_task(id, &worker, stage, &task, &target).await;
        r.end(id, passed);
        let up = Up {
            worker,
            target,
            last: id,
        };
        lock(&r.spanning).insert(name.to_string(), up);
    }
    match last.then(|| lock(&r.spanning).remove(name)).flatten() {
        Some(up) => r.stop_sandbox(&up.worker, &up.target, Some(up.last)).await && passed,
        None => passed,
    }
}

/// The tasks share one sandbox. They run one at a time, in order, each building on the one before,
/// and once one fails the rest are skipped; or, in a `parallel` stage, all at once. Starting the
/// sandbox shows in the first task's log, stopping it in the log of the last task to end. Tasks a
/// restart carried over (only a `parallel` stage has them) do not run again, but count.
pub(super) async fn run_shared_stage(
    r: &Running,
    stage: &Stage,
    sandbox: &Sandbox,
    stage_run: &StageRun,
) -> bool {
    let (carried, fresh): (Vec<_>, Vec<_>) = (stage.tasks.iter())
        .zip(&stage_run.tasks)
        .partition(|(_, task_run)| task_run.carried_from.is_some());
    let carried_passed = carried.iter().all(|(_, t)| t.state == TaskState::Pass);
    let Some((_, first)) = fresh.first() else {
        return carried_passed;
    };
    // Nothing can run when the sandbox cannot: the first task says why, and the rest are skipped.
    let none_ran = |why: String| {
        r.refuse(first.id, why);
        for (_, task_run) in fresh.iter().skip(1) {
            r.skip(task_run.id);
        }
        false
    };
    let worker = match r
        .claim(first.id, sandbox.worker.as_deref(), &sandbox.labels)
        .await
    {
        Ok(worker) => worker,
        Err(e) => return none_ran(e),
    };
    let target = match r.start_sandbox(first.id, &worker, sandbox).await {
        Ok(target) => target,
        Err(e) => return none_ran(e),
    };
    if sandbox.parallel {
        return run_together(r, &stage.name, &fresh, &worker, &target).await && carried_passed;
    }

    // The stage holds one slot of its worker from its first task to its sandbox stopping: each task
    // takes it over from the one before, whose end is recorded only then, so no other task gets
    // the worker in between.
    let mut passed = true;
    let mut ended: Option<(Uuid, bool)> = None;
    let mut last = first.id;
    for &(task, task_run) in &fresh {
        if !passed || r.is_cancelled() {
            r.skip(task_run.id);
            continue;
        }
        // The first one is running already: it started the sandbox. The rest run where it does.
        if task_run.id != first.id {
            r.boss.continue_on(&r.run, task_run.id, &worker);
        }
        if let Some((id, passed)) = ended.take() {
            r.end(id, passed);
        }
        passed = r
            .run_task(task_run.id, &worker, &stage.name, task, &target)
            .await;
        ended = Some((task_run.id, passed));
        last = task_run.id;
    }

    // A sandbox that will not stop is leaking worker resources. The stage did not pass cleanly:
    // the last task that ran fails for it.
    let stopped = r.stop_sandbox(&worker, &target, Some(last)).await;
    if let Some((id, passed)) = ended.filter(|_| stopped) {
        r.end(id, passed);
    }
    passed && stopped
}

/// The tasks of a `parallel` stage, all at once in its sandbox, which is up on `worker` already.
/// Each holds a slot of that worker while it runs (the first has one: it started the sandbox), so a
/// worker that takes fewer tasks at once runs them a few at a time, in the order the pipedef
/// defines them. A failed task fails the stage but stops none of the others.
pub(super) async fn run_together(
    r: &Running,
    stage: &str,
    tasks_here: &[(&Task, &TaskRun)],
    worker: &str,
    target: &client::Target,
) -> bool {
    let first = tasks_here.first().map(|(_, t)| t.id);
    let mut tasks = JoinSet::new();
    let mut passed = true;
    for (task, task_run) in tasks_here {
        let id = task_run.id;
        // One claim at a time, so slots go to the tasks in order, and a freed slot wakes one
        // waiting task rather than all of them.
        if Some(id) != first
            && let Err(e) = r.claim(id, Some(worker), &[]).await
        {
            passed &= r.refuse(id, e);
            continue;
        }
        let (r, stage, task) = (r.clone(), stage.to_string(), (*task).clone());
        let (worker, target) = (worker.to_string(), target.clone());
        tasks.spawn(async move {
            let passed = r.run_task(id, &worker, &stage, &task, &target).await;
            r.end(id, passed);
            (id, passed)
        });
    }
    let mut last = first;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((id, task_passed)) => {
                last = Some(id);
                passed &= task_passed;
            }
            Err(_) => passed = false,
        }
    }
    r.stop_sandbox(worker, target, last).await && passed
}

/// A task of a run, as the client sees it: its log, its artifacts, and the artifacts of the rest
/// of its run.
pub(super) struct BossTask<'a> {
    pub(super) boss: &'a Boss,
    pub(super) run: &'a Run,
    pub(super) id: Uuid,
    pub(super) cancelled: Cancelled,
    // What the task shows the boss when it fetches its checkout and artifacts. Good for as long
    // as this lives, which is as long as the task runs.
    pub(super) token: Option<String>,
}

impl<'a> BossTask<'a> {
    fn new(boss: &'a Boss, run: &'a Run, id: Uuid, cancelled: Cancelled) -> Self {
        // Without a token (no randomness to be had) the task acts as logged out, as it used to.
        let token = auth::new_token().ok().map(|(token, hash)| {
            lock(&boss.task_tokens).insert(hash, id);
            token
        });
        BossTask {
            boss,
            run,
            id,
            cancelled,
            token,
        }
    }
}

impl Drop for BossTask<'_> {
    fn drop(&mut self) {
        if let Some(token) = &self.token {
            lock(&self.boss.task_tokens).remove(&auth::token_hash(token));
        }
    }
}

impl client::TaskIo for BossTask<'_> {
    fn log(&mut self, stream: LogStream, line: String) {
        self.boss.log(self.id, stream, line);
    }

    fn log_lines(&mut self, lines: Vec<LogLine>) {
        self.boss.log_lines(self.id, lines);
    }

    fn tests(&mut self, tests: Tests) {
        let event = Event::TaskTests {
            pipeline: self.run.pipeline.clone(),
            run: self.run.id,
            task: self.id,
            tests,
        };
        let written = self
            .boss
            .record(|db| Ok((event, db.set_tests(self.id, tests)?)));
        if let Err(e) = written {
            eprintln!("could not record the tests of task {}: {e}", self.id);
        }
    }

    fn outputs(&self) -> Result<PathBuf, String> {
        task_artifacts_dir(self.id).map_err(|e| e.to_string())
    }

    fn saved(&mut self, artifact: Artifact) {
        self.boss.add_artifact(self.id, artifact);
    }

    fn run(&self) -> Option<&Run> {
        Some(self.run)
    }

    fn boss_name(&self) -> Option<&str> {
        self.boss.tls.as_ref().map(|tls| tls.name.as_str())
    }

    fn boss_port(&self) -> u16 {
        self.boss.port
    }

    fn cancelled(&self) -> Option<Cancelled> {
        Some(self.cancelled.clone())
    }

    fn boss_token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn wait_when_busy(&self) -> bool {
        true
    }
}

pub(super) async fn get_run(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Run>, Failure> {
    find_run(&boss, &id).map(Json)
}

/// Run `id`, which has to be a uuid the db knows.
pub(super) fn find_run(boss: &Boss, id: &str) -> Result<Run, Failure> {
    let id = parse_id(id)?;
    let run = boss.db().run(id).map_err(internal)?;
    run.ok_or_else(|| not_found(format!("run {id}")))
}

/// What the views of run `id` show of it, drawn now, with the pipedef as the run's commit has it:
/// JSON for the results page. 422 when the pipedef does not get that far.
pub(super) async fn get_views(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let run = find_run(&boss, &id)?;
    let broken = |e| (StatusCode::UNPROCESSABLE_ENTITY, e);
    let source = load_source(&boss, &run.pipeline, run.commit.as_ref()).await?;
    let log = |task| boss.db().log(task).map(log_text).map_err(|e| e.to_string());
    let views = pipedef::views(&source.map_err(broken)?, &run, log).await;
    let headers = [(header::CONTENT_TYPE, "application/json")];
    Ok((headers, views.map_err(broken)?).into_response())
}

fn log_text(lines: Vec<LogLine>) -> String {
    lines.into_iter().map(|l| l.line + "\n").collect()
}

pub(super) async fn run_events(
    State(boss): State<Shared>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    let snapshot = move |db: &Db| db.run(id);
    let wanted = |run: &Run, event: &Event| event.run().is_some_and(|(_, id)| id == run.id);
    live(boss, &headers, snapshot, wanted)
}

/// The files of the commit run `id` tests, as a tar stream, for its tasks to run in.
pub(super) async fn get_source(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let run = find_run(&boss, &id)?;
    let id = run.id;
    let commit = run
        .commit
        .ok_or_else(|| not_found(format!("commit: run {id} tests none")))?;
    let pipeline = &run.pipeline;
    let Some(Pipedef::Repo { repo, .. }) = boss.pipedef(pipeline) else {
        return Err(not_found(format!("repo: pipeline {pipeline} is in none")));
    };
    let mut git = boss.repo(repo)?.archive(&commit.sha).map_err(internal)?;
    let mut archive = git.spawn().map_err(internal)?;
    let stdout = archive
        .stdout
        .take()
        .ok_or_else(|| internal("git archive has no stdout"))?;
    // The stream reads stdout after this returns, so git must live on until it is done.
    spawn(async move {
        let _ = archive.wait().await;
    });
    let headers = [(header::CONTENT_TYPE, "application/x-tar")];
    Ok((headers, Body::from_stream(ReaderStream::new(stdout))).into_response())
}

/// A task's whole log as plain text, for curl.
pub(super) async fn get_task_log(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<String, Failure> {
    let id = parse_id(&id)?;
    let db = boss.db();
    if db.task(id).map_err(internal)?.is_none() {
        return Err(not_found(format!("task {id}")));
    }
    db.log(id).map(log_text).map_err(internal)
}

pub(super) async fn task_events(
    State(boss): State<Shared>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    let snapshot = move |db: &Db| {
        let Some((run, stage)) = db.task(id)? else {
            return Ok(None);
        };
        let mut tasks = run.stages.iter().flat_map(|s| &s.tasks);
        let Some(task) = tasks.find(|t| t.id == id).cloned() else {
            return Ok(None);
        };
        // A task a restart carried over shows the log and artifacts of the run that ran it.
        let ran = task.carried_from.unwrap_or(id);
        let earlier = task.carried_from.map(|from| db.task(from)).transpose()?;
        let carried_from = earlier.flatten().map(|(run, _)| run.id);
        let lines = db.log(ran)?;
        let artifacts = db.artifacts(ran)?;
        let (log_deleted, artifacts_deleted) = db.deleted(ran)?;
        Ok(Some(TaskSnapshot {
            now: now(),
            run,
            carried_from,
            log_deleted,
            artifacts_deleted,
            stage,
            task,
            lines,
            artifacts,
        }))
    };
    let wanted = |snapshot: &TaskSnapshot, event: &Event| match event {
        Event::TaskStarted { task, .. }
        | Event::TaskState { task, .. }
        | Event::TaskTests { task, .. }
        | Event::Log { task, .. }
        | Event::Artifact { task, .. } => *task == snapshot.task.id,
        Event::RunFinished { run, .. } => *run == snapshot.run.id,
        Event::RunStarted(_) | Event::Worker(_) | Event::WorkerForgotten { .. } => false,
    };
    live(boss, &headers, snapshot, wanted)
}
