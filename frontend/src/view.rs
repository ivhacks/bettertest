// Pure view functions: data in, html out. The components in main.rs own state and side effects;
// everything here can be tested by comparing exact html.

use bettertest_shared::*;
use std::collections::HashMap;
use yew::prelude::*;

pub fn header(crumbs: Html, right: Html) -> Html {
    html! {
        <header>
            <a class="home" href="/"><img src="/assets/mark.v1.svg" alt="" />{ "bettertest" }</a>
            { crumbs }
            <span class="spacer"></span>
            { right }
            <a class="page" href="/workers">{ "Workers" }</a>
            <a class="page" href="/settings">{ "Settings" }</a>
            <a class="page" href="/users">{ "Users" }</a>
        </header>
    }
}

pub fn notice(text: &Option<String>) -> Html {
    match text {
        Some(text) => html! { <pre class="notice">{ text }</pre> },
        None => html! {},
    }
}

/// In place of the results of run `id`, which the snapshot does not hold.
pub fn not_recent(id: &str) -> Html {
    notice(&Some(format!("run {id} is not one of the {RECENT_RUNS} newest")))
}

/// One tab per pipeline along the bottom, like sheets in a spreadsheet.
pub fn tabs(names: &[String], current: &str) -> Html {
    html! {
        <nav class="tabs">
            { for names.iter().map(|name| {
                let class = if name == current { "tab current" } else { "tab" };
                html! { <a class={class} href={format!("/pipeline/{name}")}>{ name }</a> }
            }) }
        </nav>
    }
}

/// How the grid lays out the task names and cells, measured in the browser (see `label_layout`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridLayout {
    // The task names tilt up by this many degrees, all the same. 0 is flat.
    pub angle: f64,
    // The height of the row of task names, in px. 0 leaves it to the browser.
    pub height: f64,
    // By stage: columns too narrow for gaps between them, so the stage's tasks run together into
    // one rectangle.
    pub dense: Vec<bool>,
}

/// How wide each stage is, relative to the others. More tasks make a stage wider, but each of
/// them narrower: a stage of 2 is √2 as wide as a stage of 1, so each task gets 71% of the single
/// one's width, not 50%.
pub fn stage_weight(tasks: usize) -> f64 {
    (tasks as f64).sqrt()
}

/// The angle and row height for task names `labels` px long, each centered on a column `columns`
/// px wide, with lines `line` px tall. Flat when every name fits its column. Otherwise every name
/// tilts by the one angle that keeps them as flat as possible while neighbours stay a full line
/// apart across, a clear gap over the letters, so they never run together into one word.
pub fn label_layout(columns: &[f64], labels: &[f64], line: f64) -> (f64, f64) {
    // Room on either side of a flat name, so neighbours never touch.
    const MARGIN: f64 = 8.0;
    let fits = columns.iter().zip(labels).all(|(c, l)| l + MARGIN <= *c);
    if fits {
        return (0.0, 0.0);
    }
    let narrowest = columns.iter().copied().fold(f64::INFINITY, f64::min);
    // Neighbouring names are parallel, one column apart: `narrowest * sin(angle)` apart across.
    let apart = line;
    let angle = (apart / narrowest).clamp(0.0, 1.0).asin();
    let tallest = labels.iter().copied().fold(0.0, f64::max);
    let height = tallest * angle.sin() + line * angle.cos() + 4.0;
    (angle.to_degrees(), height.ceil())
}

/// One row per run, newest on top. The columns are the tasks of the pipedef as it is now. A run
/// from before a task existed shows an empty cell there. A CSS grid, one track per task, each
/// stage `stage_weight` wide; stages sit further apart than tasks. `rows` are `run_row`s.
pub fn grid(pipeline: &Pipeline, layout: &GridLayout, rows: Html) -> Html {
    // Room for a running run's cancel button beside its id.
    let mut columns = String::from("88px");
    for stage in &pipeline.stages {
        let each = stage_weight(stage.tasks.len()) / stage.tasks.len().max(1) as f64;
        for _ in &stage.tasks {
            // minmax(0, …): the weights alone size the columns, never the names in them.
            columns.push_str(&format!(" minmax(0, {each:.4}fr)"));
        }
    }
    let tilted = layout.angle > 0.0;
    let names_style = (layout.height > 0.0).then(|| format!("height: {}px", layout.height));
    let name_style = tilted.then(|| format!("transform: rotate(-{:.2}deg)", layout.angle));
    html! {
        <table class={classes!("grid", tilted.then_some("tilted"))} style={format!("grid-template-columns: {columns}")}>
            <thead>
                <tr>
                    <th></th>
                    { for pipeline.stages.iter().map(|stage| html! {
                        <th class="stage" style={format!("grid-column: span {}", stage.tasks.len())}>
                            <span>{ &stage.name }</span>
                        </th>
                    }) }
                </tr>
                <tr>
                    <th style={names_style.clone()}></th>
                    { for pipeline.stages.iter().enumerate().flat_map(|(s, stage)| {
                        stage.tasks.iter().enumerate().map(move |(t, task)| (s, t, task))
                    }).map(|(s, t, task)| html! {
                        <th class={classes!("task", cell_classes(layout, s, t))} style={names_style.clone()}>
                            <span style={name_style.clone()}>{ &task.name }</span>
                        </th>
                    }) }
                </tr>
            </thead>
            <tbody>
                { rows }
            </tbody>
        </table>
    }
}

/// `first` opens a stage, after the wider gap between stages; `dense` packs a stage's tasks.
fn cell_classes(layout: &GridLayout, stage: usize, task: usize) -> Classes {
    let dense = layout.dense.get(stage).copied().unwrap_or(false);
    classes!((task == 0).then_some("first"), dense.then_some("dense"))
}

pub fn run_row(
    pipeline: &Pipeline,
    run: &Run,
    layout: &GridLayout,
    cancel: &Callback<String>,
) -> Html {
    // By (stage, task): a stage can have thousands of tasks, too many to search for each cell.
    let task_runs: HashMap<(&str, &str), &TaskRun> = run
        .stages
        .iter()
        .flat_map(|s| {
            s.tasks
                .iter()
                .map(move |t| ((s.name.as_str(), t.name.as_str()), t))
        })
        .collect();
    let cells = pipeline.stages.iter().enumerate().flat_map(|(s, stage)| {
        let task_runs = &task_runs;
        stage.tasks.iter().enumerate().map(move |(t, task)| {
            let task_run = task_runs.get(&(stage.name.as_str(), task.name.as_str()));
            let place = cell_classes(layout, s, t);
            match task_run {
                Some(t) => {
                    // A running task's time would go stale in a tooltip, so only finished ones say.
                    let mut title = match (t.state.is_over(), t.runtime(0)) {
                        (true, Some(ms)) => format!("{} in {}", t.state.as_str(), duration(ms)),
                        _ => t.state.as_str().to_string(),
                    };
                    // A dense stage hides its tasks' names, so the cell says whose it is.
                    if layout.dense.get(s).copied().unwrap_or(false) {
                        title = format!("{}: {title}", task.name);
                    }
                    let carried = t.carried_from.is_some();
                    if carried {
                        title.push_str(", carried over");
                    }
                    html! {
                        <td class={classes!("cell", t.state.as_str(), carried.then_some("carried"), place)}>
                            <a href={format!("/task/{}", t.id)} title={title}></a>
                        </td>
                    }
                }
                None => html! { <td class={classes!("cell", place)}></td> },
            }
        })
    });
    let commit = run.commit.as_ref().map(Commit::label);
    html! {
        <tr>
            <th class={classes!("run", run.state.as_str())} title={commit}>
                { cancel_button(run, cancel) }<a href={run_href(run)}>{ run.short_id() }</a>
            </th>
            { for cells }
        </tr>
    }
}

/// The red X before a running run's id: it cancels the run, all of it, at once. `cancel` gets the
/// run's id. Nothing once the run is over.
fn cancel_button(run: &Run, cancel: &Callback<String>) -> Html {
    if run.state != RunState::Running {
        return html! {};
    }
    let id = run.id.to_string();
    html! {
        <button class="cancel" title="Cancel this run: every task, now" aria-label="Cancel run"
            onclick={cancel.reform(move |_| id.clone())}>
            <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2 2L8 8M8 2L2 8" /></svg>
        </button>
    }
}

/// The path to a run, at the left of the header: its pipeline, the run and what it tested.
fn run_crumbs(run: &Run, cancel: &Callback<String>) -> Html {
    html! {
        <>
            <a href={format!("/pipeline/{}", run.pipeline)}>{ &run.pipeline }</a>
            <span class="run">
                { cancel_button(run, cancel) }<a href={run_href(run)}>{ run.short_id() }</a>
            </span>
            { for run.commit.iter().map(|c| html! { <span class="commit">{ c.label() }</span> }) }
        </>
    }
}

pub fn task_crumbs(snapshot: &TaskSnapshot, cancel: &Callback<String>) -> Html {
    html! {
        <>
            { run_crumbs(&snapshot.run, cancel) }
            <span>{ &snapshot.stage }</span>
            <span>{ &snapshot.task.name }</span>
            <span class={classes!("state", snapshot.task.state.as_str())}>{ snapshot.task.state.as_str() }</span>
        </>
    }
}

/// Where a run's results are: on its pipeline's page, in place of the grid.
fn run_href(run: &Run) -> String {
    format!("/pipeline/{}?run={}", run.pipeline, run.id)
}

pub fn run_page_crumbs(run: &Run, cancel: &Callback<String>) -> Html {
    html! {
        <>
            { run_crumbs(run, cancel) }
            <span class={classes!("state", run.state.as_str())}>{ run.state.as_str() }</span>
        </>
    }
}

/// What the tasks of a run came to: how many of them passed, failed and so on, and how many tests
/// they ran, then every task, failures first and the slowest next. A task that failed without
/// failing a test errored: it crashed, timed out, or never said what its tests did.
pub fn results(run: &Run) -> Html {
    let outcome = |task: &TaskRun| match (task.state, task.tests) {
        (TaskState::Fail, Some(tests)) if tests.failed > 0 => "fail",
        (TaskState::Fail, _) => "error",
        (state, _) => state.as_str(),
    };
    const OUTCOMES: [&str; 6] = ["fail", "error", "running", "pending", "skipped", "pass"];
    let rank = |task: &TaskRun| OUTCOMES.iter().position(|o| *o == outcome(task));
    let took = |task: &TaskRun| task.state.is_over().then(|| task.runtime(0)).flatten();
    let mut tasks: Vec<(&str, &TaskRun)> = (run.stages.iter())
        .flat_map(|stage| stage.tasks.iter().map(|task| (stage.name.as_str(), task)))
        .collect();
    tasks.sort_by_key(|(_, task)| (rank(task), std::cmp::Reverse(took(task))));
    let totals = OUTCOMES.iter().filter_map(|o| {
        let n = tasks.iter().filter(|(_, task)| outcome(task) == *o).count();
        (n > 0).then(|| html! { <span class={classes!("state", *o)}>{ format!("{n} {o}") }</span> })
    });
    let tests = tasks.iter().filter_map(|(_, task)| task.tests);
    let (total, failed) = tests.fold((0, 0), |(t, f), tests| (t + tests.total, f + tests.failed));
    let rows = tasks.iter().map(|(stage, task)| {
        html! {
            <tr>
                <td><span class={classes!("state", outcome(task))}>{ outcome(task) }</span></td>
                <td>{ *stage }</td>
                <td><a href={format!("/task/{}", task.id)}>{ &task.name }</a></td>
                <td class="number">{ for took(task).map(duration) }</td>
                <td class="number">{ for task.tests.map(|t| count(t.total)) }</td>
                <td class="number">{ for task.tests.map(|t| count(t.failed)) }</td>
            </tr>
        }
    });
    html! {
        <>
            <p class="totals">
                { for totals }
                <span>{ format!("{} tests, {} failed", count(total), count(failed)) }</span>
            </p>
            <table class="results">
                <tr><th></th><th>{ "Stage" }</th><th>{ "Task" }</th><th>{ "Took" }</th><th>{ "Tests" }</th><th>{ "Failed" }</th></tr>
                { for rows }
            </table>
        </>
    }
}

/// "1,031,284": a count, for people to read.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let groups: Vec<&str> = (digits.as_bytes().rchunks(3).rev())
        .filter_map(|group| std::str::from_utf8(group).ok())
        .collect();
    groups.join(",")
}

/// Where and how long the task ran (so far, by `now`), for the right end of the header. Nothing
/// before it has started.
pub fn task_facts(task: &TaskRun, now: u64) -> Html {
    let worker = task.worker.as_ref().map(|worker| {
        let host = worker
            .strip_prefix("http://")
            .or_else(|| worker.strip_prefix("https://"))
            .unwrap_or(worker);
        html! { <span class="worker" title={format!("worker {worker}")}>{ "on " }{ host }</span> }
    });
    let runtime = task
        .runtime(now)
        .map(|ms| html! { <span class="runtime">{ duration(ms) }</span> });
    html! { <>{ for worker }{ for runtime }</> }
}

/// What a rerun can run again, from the task page. Each button reruns that much of the run, and
/// every stage after it.
#[derive(Clone, Copy, PartialEq)]
pub enum Rerun {
    Task,
    Stage,
    Run,
}

/// The rerun buttons for a task of `run` in `stage`. A stage after one that failed never ran, so
/// there is nothing to rerun from it: only the whole run.
pub fn rerun_buttons(run: &Run, stage: &str, rerun: &Callback<Rerun>) -> Html {
    let button = |what: Rerun, label: &str| {
        let onclick = rerun.reform(move |_| what);
        html! { <button {onclick}>{ label }</button> }
    };
    let from_here = run.passed_before(stage).then(|| {
        html! {
            <>
                { button(Rerun::Task, "Rerun Task") }
                { button(Rerun::Stage, "Rerun Stage") }
            </>
        }
    });
    html! {
        <>
            { for from_here }
            { button(Rerun::Run, "Rerun Run") }
        </>
    }
}

/// For a task a rerun carried over: where its result, log and artifacts come from.
pub fn carried(snapshot: &TaskSnapshot) -> Html {
    let (Some(task), Some(run)) = (snapshot.task.carried_from, snapshot.carried_from) else {
        return html! {};
    };
    let short: String = run.simple().to_string().chars().take(6).collect();
    html! {
        <p class="note">
            { "Carried over from run " }<a href={format!("/task/{task}")}>{ short }</a>
            { ". This rerun did not run it again." }
        </p>
    }
}

/// Why a task's log or artifacts are gone, once retention deleted them. A task that never started
/// had no log to delete.
pub fn retention_note(snapshot: &TaskSnapshot) -> Html {
    let log = snapshot.log_deleted.is_some() && snapshot.task.started.is_some();
    let what = match (log, snapshot.artifacts_deleted.is_some()) {
        (true, true) => "log and artifacts",
        (true, false) => "log",
        (false, true) => "artifacts",
        (false, false) => return html! {},
    };
    html! {
        <p class="note">
            { format!("Retention deleted this task's {what}. ") }
            <a href="/settings">{ "Settings" }</a>{ " say how long they are kept." }
        </p>
    }
}

/// The settings form. Empty means forever.
pub fn settings(
    settings: &Settings,
    artifacts: &NodeRef,
    logs: &NodeRef,
    onsubmit: Callback<SubmitEvent>,
) -> Html {
    let days = |value: Option<u32>| value.map(|d| d.to_string()).unwrap_or_default();
    html! {
        <form class="settings" {onsubmit}>
            <label>
                { "Keep task artifacts for " }
                <input ref={artifacts} type="number" min="1" max="36500" placeholder="forever" value={days(settings.artifact_days)} />
                { " days" }
            </label>
            <label>
                { "Keep task logs for " }
                <input ref={logs} type="number" min="1" max="36500" placeholder="forever" value={days(settings.log_days)} />
                { " days" }
            </label>
            <p class="hint">
                { "Counted from when each task ends. Empty keeps them forever. The boss deletes what is past its days once an hour, and as soon as you save." }
            </p>
            <button class="primary" type="submit">{ "Save" }</button>
        </form>
    }
}

/// Who is logged in, with a way out; or a way in, for the logged-out user.
pub fn account(me: &Me, logout: Callback<MouseEvent>, passkey: Callback<MouseEvent>) -> Html {
    if me.logged_in {
        html! {
            <>
                <span class="me">{ &me.name }</span>
                <button class="small" onclick={passkey}>{ "Add Passkey" }</button>
                <button class="small" onclick={logout}>{ "Log Out" }</button>
            </>
        }
    } else {
        html! { <a class="page" href="/login">{ "Log In" }</a> }
    }
}

/// One row per user, logged out first, with a column per way of logging in. `change` gets a user's
/// id and its new role; `password`, `token`, `certificate`, `passkey` and `remove` a user's id;
/// `revoke` a certificate's or passkey's.
pub struct UserActions {
    pub change: Callback<(i64, Role)>,
    pub password: Callback<i64>,
    pub token: Callback<i64>,
    pub certificate: Callback<i64>,
    pub passkey: Callback<i64>,
    pub revoke: Callback<i64>,
    pub remove: Callback<i64>,
}

pub fn users(users: &[User], actions: &UserActions) -> Html {
    let UserActions {
        change,
        password,
        token,
        certificate,
        passkey,
        revoke,
        remove,
    } = actions;
    // Each with its label, and when it expires if it does.
    let credentials = |listed: &mut dyn Iterator<Item = (i64, &String, Option<String>)>| {
        let listed: Vec<Html> = listed
            .map(|(credential, label, title)| html! {
                <span class="cert" {title}>
                    { label }
                    <button class="small" onclick={revoke.reform(move |_| credential)}>{ "Revoke" }</button>
                </span>
            })
            .collect();
        html! { <td class="certs">if listed.is_empty() { { "-" } }{ for listed }</td> }
    };
    let count = |n: u32| {
        if n == 0 {
            "-".to_string()
        } else {
            n.to_string()
        }
    };
    html! {
        <table class="users">
            <thead>
                <tr>
                    <th>{ "User" }</th>
                    <th>{ "Role" }</th>
                    <th>{ "Password" }</th>
                    <th>{ "Certificate" }</th>
                    <th>{ "Passkey" }</th>
                    <th>{ "API Tokens" }</th>
                    <th></th>
                </tr>
            </thead>
            <tbody>
                { for users.iter().map(|u| {
                    let id = u.id;
                    let onchange = change.reform(move |e: yew::Event| {
                        let value = e.target_unchecked_into::<web_sys::HtmlSelectElement>().value();
                        (id, Role::parse(&value).unwrap_or(Role::None))
                    });
                    let roles = [Role::None, Role::Read, Role::Write, Role::Admin];
                    let actions = (!u.builtin).then(|| html! {
                        <>
                            <button class="small" onclick={password.reform(move |_| id)}>{ "Set Password" }</button>
                            <button class="small" onclick={token.reform(move |_| id)}>{ "New Token" }</button>
                            if u.certs.len() < MOST_CERTIFICATES {
                                <button class="small" onclick={certificate.reform(move |_| id)}>{ "New Certificate" }</button>
                            }
                            if u.keys.len() < MOST_PASSKEYS {
                                <button class="small" onclick={passkey.reform(move |_| id)}>{ "New Passkey" }</button>
                            }
                            <button class="small" onclick={remove.reform(move |_| id)}>{ "Remove" }</button>
                        </>
                    });
                    html! {
                        <tr class={classes!(u.builtin.then_some("builtin"))}>
                            <td class="name">{ &u.name }</td>
                            <td>
                                <select {onchange}>
                                    { for roles.iter().map(|r| html! {
                                        <option value={r.as_str()} selected={*r == u.role}>{ r.as_str() }</option>
                                    }) }
                                </select>
                            </td>
                            if u.builtin {
                                <td colspan="4" class="hint">{ "Anyone who has not logged in" }</td>
                            } else {
                                <td>{ if u.passwords > 0 { "set" } else { "-" } }</td>
                                { credentials(&mut u.certs.iter().map(|c| (c.id, &c.label, Some(format!("expires {}", c.expires))))) }
                                { credentials(&mut u.keys.iter().map(|k| (k.id, &k.label, None))) }
                                <td>{ count(u.tokens) }</td>
                            }
                            <td class="actions">{ for actions }</td>
                        </tr>
                    }
                }) }
            </tbody>
        </table>
    }
}

/// Whether the boss speaks HTTPS, which `--cert` and `--key` decide when it starts, so the page only
/// says so. A certificate that expires within 30 days says that too: renewing it takes someone with
/// the site's certificate authority, and a restart.
pub fn https(https: &Option<Https>) -> Html {
    let Some(https) = https else {
        return html! {
            <section class="https">
                <h2>{ "HTTPS" }</h2>
                <p>{ "Off. Start the boss with --cert and --key to turn it on." }</p>
            </section>
        };
    };
    let soon = (https.days_left < 30).then(|| {
        let days = match https.days_left {
            1 => "1 day".to_string(),
            n => format!("{n} days"),
        };
        html! {
            <pre class="notice">{ format!("The certificate expires in {days}. Restart the boss with a new one before then.") }</pre>
        }
    });
    html! {
        <section class="https">
            <h2>{ "HTTPS" }</h2>
            <p>{ format!("On, with a certificate for {}, expiring {}.", https.names.join(", "), https.expires) }</p>
            { for soon }
        </section>
    }
}

/// Asks before forgetting worker `url`: a pop-up window over the page.
pub fn forget_window(
    url: &str,
    cancel: Callback<MouseEvent>,
    forget: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class="backdrop">
            <div class="window" role="dialog">
                <h2>{ "Forget Worker?" }</h2>
                <p>{ format!("{url} stops getting tasks. A pipedef that names it will add it back.") }</p>
                <div class="buttons">
                    <button onclick={cancel}>{ "Cancel" }</button>
                    <button class="primary" onclick={forget}>{ "Forget" }</button>
                </div>
            </div>
        </div>
    }
}

/// Download links for the files the task published. Nothing at all when there are none.
pub fn artifacts(task: &TaskRun, artifacts: &[Artifact]) -> Html {
    if artifacts.is_empty() {
        return html! {};
    }
    html! {
        <nav class="artifacts">
            { for artifacts.iter().map(|a| html! {
                <a href={format!("/api/task/{}/artifact/{}", task.id, a.name)} download="">
                    { &a.name }<span class="size">{ size(a.size) }</span>
                </a>
            }) }
        </nav>
    }
}

/// One row per worker the boss knows, connected or not, with what runs on it now. `now` is the
/// boss's clock, for how long each has been up or down.
pub fn workers(workers: &[Worker], now: u64, forget: &Callback<String>) -> Html {
    if workers.is_empty() {
        return html! {
            <p class="empty">{ "No workers yet. A pipedef that names one adds it, or add one above." }</p>
        };
    }
    html! {
        <table class="workers">
            <thead>
                <tr>
                    <th>{ "worker" }</th>
                    <th>{ "host" }</th>
                    <th>{ "arch" }</th>
                    <th>{ "labels" }</th>
                    <th>{ "version" }</th>
                    <th>{ "busy" }</th>
                    <th>{ "for" }</th>
                    <th>{ "running" }</th>
                    <th></th>
                </tr>
            </thead>
            <tbody>
                { for workers.iter().map(|w| worker_row(w, now, forget)) }
            </tbody>
        </table>
    }
}

fn worker_row(worker: &Worker, now: u64, forget: &Callback<String>) -> Html {
    let url = worker.url.clone();
    let onclick = forget.reform(move |_| url.clone());
    let about = match &worker.info {
        Some(info) => html! {
            <>
                <td>{ &info.hostname }</td>
                <td>{ info.arch.as_str() }</td>
                <td>{ info.labels.join(", ") }</td>
                <td>{ &info.version }</td>
                <td class="busy">{ format!("{}/{}", worker.tasks.len(), info.capacity) }</td>
            </>
        },
        None => {
            let problem = worker.problem.as_deref().unwrap_or("connecting");
            html! { <td colspan="5" class="problem">{ problem }</td> }
        }
    };
    let state = if worker.info.is_some() { "up" } else { "down" };
    html! {
        <tr class={state}>
            <td class="url">{ &worker.url }</td>
            { about }
            <td class="since">{ format!("{state} {}", duration(now.saturating_sub(worker.since))) }</td>
            <td class="tasks">
                { for worker.tasks.iter().map(|t| {
                    let run: String = t.run.simple().to_string().chars().take(6).collect();
                    html! {
                        <a href={format!("/task/{}", t.id)}>
                            { format!("{} {run} {}.{}", t.pipeline, t.stage, t.task) }
                        </a>
                    }
                }) }
            </td>
            <td><button class="small" {onclick}>{ "Forget" }</button></td>
        </tr>
    }
}

fn size(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for bigger in ["KB", "MB", "GB", "TB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = bigger;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

pub fn log(lines: &[LogLine]) -> Html {
    html! {
        <pre class="log">
            { for lines.iter().map(|l| html! {
                <div class={l.stream.as_str()}>{ &l.line }</div>
            }) }
        </pre>
    }
}

#[cfg(test)]
#[path = "../test/view.rs"]
mod tests;
