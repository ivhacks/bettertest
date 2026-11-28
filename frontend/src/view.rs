// Pure view functions: data in, html out. The components in main.rs own state and side effects;
// everything here can be tested by comparing exact html.

use bettertest_shared::*;
use std::collections::HashMap;
use yew::prelude::*;

/// The top of every page, one row: the mark, what this page is about and can do, then an icon for
/// each page and the account. On a phone the page's own things take a line below the icons.
pub fn header(crumbs: Html, actions: Html, account: Html) -> Html {
    let context = (crumbs != Html::default() || actions != Html::default()).then(|| {
        html! { <span class="context">{ crumbs }<span class="actions">{ actions }</span></span> }
    });
    html! {
        <header>
            <a class="home" href="/"><img src="/assets/mark.v1.svg" alt="bettertest" /></a>
            { for context }
            <nav class="links">
                { icon("Pipelines", Some("/"), html! {
                    <><rect x="3.5" y="4.5" width="17" height="15" /><path d="M3.5 9.5h17M3.5 14.5h17M9.5 9.5v10M15 9.5v10" /></>
                }) }
                { icon("Workers", Some("/workers"), html! {
                    <><rect x="3.5" y="4.5" width="17" height="6" /><rect x="3.5" y="13.5" width="17" height="6" /><path d="M7 7.5h2M7 16.5h2" /></>
                }) }
                { icon("Settings", Some("/settings"), html! {
                    <><path d="M3.5 7.5h17M3.5 16.5h17" /><circle cx="9" cy="7.5" r="2.5" fill="var(--background)" /><circle cx="15" cy="16.5" r="2.5" fill="var(--background)" /></>
                }) }
                { icon("Users", Some("/users"), html! {
                    <><circle cx="9" cy="8.5" r="3" /><path d="M3 19.5c1-3 3-4.5 6-4.5s5 1.5 6 4.5" /><path d="M15 5.8a3 3 0 0 1 0 5.4M17.5 15.3c1.6.6 2.7 2 3.3 4.2" /></>
                }) }
                { account }
            </nav>
        </header>
    }
}

/// One of the header's icons: a 24px line drawing in a 44px square that lights up and presses like
/// a button (`.icon` in style.css). A link to `href`, or with none, the button that opens the
/// account menu.
fn icon(name: &str, href: Option<&str>, shapes: Html) -> Html {
    let drawing = html! {
        <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
            { shapes }
        </svg>
    };
    let name = name.to_string();
    match href {
        Some(href) => html! {
            <a class="icon" href={href.to_string()} title={name.clone()} aria-label={name}>{ drawing }</a>
        },
        None => html! {
            <button class="icon" popovertarget="account" title={name.clone()} aria-label={name}>{ drawing }</button>
        },
    }
}

pub fn notice(text: &Option<String>) -> Html {
    html! { { for text.iter().map(|text| html! { <pre class="notice">{ text }</pre> }) } }
}

/// In place of the results of run `id`, which the snapshot does not hold.
pub fn not_recent(id: &str) -> Html {
    let why = format!("run {id} is not one of the {RECENT_RUNS} newest");
    notice(&Some(why))
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

/// How the grid lays out the stage and task names and cells, measured in the browser (see
/// `label_layout`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridLayout {
    pub stages: Tilt,
    pub tasks: Tilt,
    // By stage: columns too narrow for gaps between them, so the stage's tasks run together into
    // one rectangle.
    pub dense: Vec<bool>,
}

/// How one row of names lies.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tilt {
    // The names tilt up by this many degrees, all the same. 0 is flat.
    pub angle: f64,
    // The height of the row, in px. 0 leaves it to the browser.
    pub height: f64,
}

/// How wide each stage is, relative to the others. More tasks make a stage wider, but each of
/// them narrower: a stage of 2 is √2 as wide as a stage of 1, so each task gets 71% of the single
/// one's width, not 50%.
pub fn stage_weight(tasks: usize) -> f64 {
    (tasks as f64).sqrt()
}

/// How names `labels` px long lie, each centered on a column `columns` px wide, with lines `line`
/// px tall; a name a dense stage hides is None. Flat when every name fits its column. Otherwise
/// every name tilts by the one angle that keeps them as flat as possible while neighbours stay a
/// full line apart across, a clear gap over the letters, so they never run together into one word,
/// and none reaches past the grid's right edge.
pub fn label_layout(columns: &[f64], labels: &[Option<f64>], line: f64) -> Tilt {
    // Room on either side of a name, so neighbours never touch.
    const MARGIN: f64 = 8.0;
    let shown = || (columns.iter().zip(labels)).filter_map(|(c, l)| Some((*c, (*l)?)));
    if shown().all(|(c, l)| l + MARGIN <= c) {
        return Tilt::default();
    }
    let narrowest = shown().map(|(c, _)| c).fold(f64::INFINITY, f64::min);
    // Neighbouring names are parallel, one column apart: `narrowest * sin(angle)` apart across.
    let mut angle = (line / narrowest).clamp(0.0, 1.0).asin();
    // A name rises to the right from the middle of its column, `label * cos(angle)` across.
    let mut room = -MARGIN / 2.0;
    for (column, label) in columns.iter().zip(labels).rev() {
        room += column / 2.0;
        angle = angle.max(label.map_or(0.0, |label| (room / label).clamp(0.0, 1.0).acos()));
        room += column / 2.0;
    }
    let tallest = shown().map(|(_, l)| l).fold(0.0, f64::max);
    let height = tallest * angle.sin() + line * angle.cos() + 4.0;
    Tilt {
        angle: angle.to_degrees(),
        height: height.ceil(),
    }
}

/// One row per run, newest on top. The columns are the tasks of the pipedef as it is now. A run
/// from before a task existed shows an empty cell there. A CSS grid, one track per task, each
/// stage `stage_weight` wide; stages sit further apart than tasks. `rows` are rows whose cells
/// `run_row` makes.
pub fn grid(pipeline: &Pipeline, layout: &GridLayout, rows: Html) -> Html {
    // The run ids' own width: a running run is cancelled from its results or a task's page.
    let mut columns = String::from("max-content");
    for stage in &pipeline.stages {
        let each = stage_weight(stage.tasks.len()) / stage.tasks.len().max(1) as f64;
        for _ in &stage.tasks {
            // minmax(0, …): the weights alone size the columns, never the names in them.
            columns.push_str(&format!(" minmax(0, {each:.4}fr)"));
        }
    }
    let (stages, tasks) = (layout.stages, layout.tasks);
    html! {
        <table class="grid" style={format!("grid-template-columns: {columns}")}>
            <style>{ dense_rule(pipeline, layout) }</style>
            <thead>
                <tr class={stages.class()}>
                    <th class="run" style={stages.height()}></th>
                    { for pipeline.stages.iter().map(|stage| html! {
                        <th class="stage" style={format!("grid-column: span {0}; --tasks: {0}", stage.tasks.len())}>
                            <span style={stages.rotate()}>{ &stage.name }</span>
                        </th>
                    }) }
                </tr>
                <tr class={tasks.class()}>
                    <th style={tasks.height()}></th>
                    { for pipeline.stages.iter().enumerate().flat_map(|(s, stage)| {
                        stage.tasks.iter().enumerate().map(move |(t, task)| (s, t, task))
                    }).map(|(s, t, task)| html! {
                        <th class={classes!("task", cell_classes(layout, s, t))} style={tasks.height()}>
                            <span style={tasks.rotate()}>{ &task.name }</span>
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

impl Tilt {
    fn class(&self) -> Option<&'static str> {
        (self.angle > 0.0).then_some("tilted")
    }

    fn height(&self) -> Option<String> {
        (self.height > 0.0).then(|| format!("height: {}px", self.height))
    }

    fn rotate(&self) -> Option<String> {
        (self.angle > 0.0).then(|| format!("transform: rotate(-{:.2}deg)", self.angle))
    }
}

/// `first` opens a stage, after the wider gap between stages; `dense` packs a stage's tasks.
fn cell_classes(layout: &GridLayout, stage: usize, task: usize) -> Classes {
    let dense = layout.dense.get(stage).copied().unwrap_or(false);
    classes!((task == 0).then_some("first"), dense.then_some("dense"))
}

/// The cells of one run's row of the grid, as HTML for the row's innerHTML. A row is the run's id,
/// then a cell per task of the pipedef as it is now. Built as text and handed over in one piece,
/// which a browser takes in far quicker than a node at a time: a page of 50 runs of 200 tasks is
/// 10 000 cells. Nothing here depends on how the names are laid out, so measuring them never
/// draws the rows again.
pub fn run_row(pipeline: &Pipeline, run: &Run) -> String {
    use std::fmt::Write;
    // By (stage, task): a stage can have thousands of tasks, too many to search for each cell.
    let task_runs: HashMap<(&str, &str), &TaskRun> = (run.stages.iter())
        .flat_map(|s| (s.tasks.iter()).map(move |t| ((s.name.as_str(), t.name.as_str()), t)))
        .collect();
    // Writing to a String cannot fail, so the results of write! are ignored.
    let tasks: usize = pipeline.stages.iter().map(|stage| stage.tasks.len()).sum();
    let mut row = String::with_capacity(160 * (1 + tasks));
    let _ = write!(row, r#"<th class="run {}""#, run.state.as_str());
    if let Some(commit) = &run.commit {
        row.push_str(r#" title=""#);
        escape(&mut row, &commit.label());
        row.push('"');
    }
    row.push_str(r#"><a href=""#);
    escape(&mut row, &run_href(run));
    let _ = write!(row, r#"">{}</a></th>"#, run.short_id());
    for stage in &pipeline.stages {
        for (t, task) in stage.tasks.iter().enumerate() {
            let first = if t == 0 { " first" } else { "" };
            let Some(task_run) = task_runs.get(&(stage.name.as_str(), task.name.as_str())) else {
                let _ = write!(row, r#"<td class="cell{first}"></td>"#);
                continue;
            };
            let state = task_run.state.as_str();
            let carried = task_run.carried_from.is_some();
            let carried_class = if carried { " carried" } else { "" };
            let _ = write!(row, r#"<td class="cell {state}{carried_class}{first}">"#);
            // A skipped task never ran: it has no page worth opening.
            if task_run.state != TaskState::Skipped {
                // The cell says whose it is: a dense stage hides its tasks' names. A running
                // task's time would go stale in a tooltip, so only finished ones say.
                let _ = write!(row, r#"<a href="/task/{}" title=""#, task_run.id);
                escape(&mut row, &task.name);
                let _ = write!(row, ": {state}");
                if let (true, Some(ms)) = (task_run.state.is_over(), task_run.runtime(0)) {
                    let _ = write!(row, " in {}", duration(ms));
                }
                if carried {
                    row.push_str(", carried over");
                }
                row.push_str(r#"" draggable="false"></a>"#);
            }
            row.push_str("</td>");
        }
    }
    row
}

/// Appends `text` to `html` as it can stand there, in an attribute or between tags.
fn escape(html: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' => html.push_str("&quot;"),
            c => html.push(c),
        }
    }
}

/// The rule packing the tasks of each dense stage together in the rows (the header's task names
/// carry a `dense` class instead): every cell of the stage but its first, which keeps the gap
/// before the stage. The rows' cells carry no classes that depend on the layout, so a new layout
/// changes this rule, not the rows.
fn dense_rule(pipeline: &Pipeline, layout: &GridLayout) -> String {
    // Each row's first child is the run's id, so the first task is child 2.
    let mut column = 2;
    let mut selectors = vec![];
    for (s, stage) in pipeline.stages.iter().enumerate() {
        let tasks = stage.tasks.len();
        if layout.dense.get(s).copied().unwrap_or(false) && tasks > 1 {
            selectors.push(format!(
                ".grid tbody td:nth-child(n+{}):nth-child(-n+{})",
                column + 1,
                column + tasks - 1
            ));
        }
        column += tasks;
    }
    match selectors.is_empty() {
        true => String::new(),
        false => format!("{} {{ padding-left: 0 }}", selectors.join(", ")),
    }
}

/// The path to a run, at the left of the header: its pipeline, the run and what it tested. A
/// running run's id has a red X before it, which cancels the run, all of it, at once: `cancel`
/// gets the run's id.
fn run_crumbs(run: &Run, cancel: &Callback<String>) -> Html {
    let id = run.id;
    html! {
        <>
            <a href={format!("/pipeline/{}", run.pipeline)}>{ &run.pipeline }</a>
            <span class="run">
                if run.state == RunState::Running {
                    <button class="cancel" title="Cancel this run: every task, now" aria-label="Cancel run"
                        onclick={cancel.reform(move |_| id.to_string())}>
                        <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2 2L8 8M8 2L2 8" /></svg>
                    </button>
                }
                <a href={run_href(run)}>{ run.short_id() }</a>
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
pub fn results(run: &Run, views: Html) -> Html {
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
                <td class="number" data-label="took">{ for took(task).map(duration) }</td>
                <td class="number" data-label="tests">{ for task.tests.map(|t| count(t.total)) }</td>
                <td class="number" data-label="failed">{ for task.tests.map(|t| count(t.failed)) }</td>
            </tr>
        }
    });
    html! {
        <>
            <p class="totals">
                { for totals }
                <span>{ format!("{} tests, {} failed", count(total), count(failed)) }</span>
            </p>
            { views }
            <table class="results">
                <thead>
                    <tr><th></th><th>{ "Stage" }</th><th>{ "Task" }</th><th>{ "Took" }</th><th>{ "Tests" }</th><th>{ "Failed" }</th></tr>
                </thead>
                <tbody>{ for rows }</tbody>
            </table>
        </>
    }
}

/// What one of the pipedef's views shows of a run (views_json in the library): a Plotly figure,
/// an HTML artifact, or why it could not draw.
#[derive(Clone, PartialEq, serde::Deserialize)]
pub struct View {
    pub name: String,
    pub wide: bool,
    pub figure: Option<serde_json::Value>,
    pub page: Option<String>,
    pub error: Option<String>,
}

/// The views of a run, above its tasks. Drawing a figure takes a component: `figure` makes one.
pub fn views(views: &[View], figure: impl Fn(&serde_json::Value) -> Html) -> Html {
    if views.is_empty() {
        return html! {};
    }
    let shown = |view: &View| match (&view.error, &view.figure, &view.page) {
        (Some(error), _, _) => html! { <pre class="notice">{ error }</pre> },
        (None, Some(drawn), _) => figure(drawn),
        (None, None, Some(page)) => {
            html! { <iframe src={page.clone()} sandbox="allow-scripts" title={view.name.clone()}></iframe> }
        }
        (None, None, None) => html! {},
    };
    html! {
        <section class="views">
            { for views.iter().map(|view| html! {
                <figure class={classes!("view", view.wide.then_some("wide"))}>
                    <figcaption>{ view.name.replace('_', " ") }</figcaption>
                    { shown(view) }
                </figure>
            }) }
        </section>
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

/// What a restart can run again, from the task page. Each button restarts that much of the run, and
/// every stage after it.
#[derive(Clone, Copy, PartialEq)]
pub enum Restart {
    Task,
    Stage,
    Run,
}

/// The restart buttons for a task of `run` in `stage`. A stage after one that failed never ran, so
/// there is nothing to restart from it: only the whole run.
pub fn restart_buttons(run: &Run, stage: &str, restart: &Callback<Restart>) -> Html {
    let button = |what: Restart, label: &str| {
        let onclick = restart.reform(move |_| what);
        html! { <button {onclick}>{ label }</button> }
    };
    html! {
        <>
            if run.passed_before(stage) {
                { button(Restart::Task, "Restart Task") }
                { button(Restart::Stage, "Restart Stage") }
            }
            { button(Restart::Run, "Restart Run") }
        </>
    }
}

/// For a task a restart carried over: where its result, log and artifacts come from.
pub fn carried(snapshot: &TaskSnapshot) -> Html {
    let (Some(task), Some(run)) = (snapshot.task.carried_from, snapshot.carried_from) else {
        return html! {};
    };
    html! {
        <p class="note">
            { "Carried over from run " }<a href={format!("/task/{task}")}>{ short_id(run) }</a>
            { ", not started again." }
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

/// The header's last icon, a person: for the logged-out user a link to log in, for anyone else the
/// button that opens their `menu`.
pub fn account(me: &Me) -> Html {
    let person = html! { <><circle cx="12" cy="8" r="3.5" /><path d="M5 20c1.3-3.3 3.7-5 7-5s5.7 1.7 7 5" /></> };
    match me.logged_in {
        true => icon(&format!("Account, {}", me.name), None, person),
        false => icon("Log In", Some("/login"), person),
    }
}

/// Who is logged in, with a way out: the popover the account icon opens. One for every page, since
/// pages stay in the document, hidden, once left.
pub fn menu(me: &Me, logout: Callback<MouseEvent>, passkey: Callback<MouseEvent>) -> Html {
    if !me.logged_in {
        return html! {};
    }
    html! {
        <span id="account" class="menu" popover="auto">
            <span class="me">{ &me.name }</span>
            <button class="small" onclick={passkey}>{ "Add Passkey" }</button>
            <button class="small" onclick={logout}>{ "Log Out" }</button>
        </span>
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
    // Each with its label, and when it expires if it does.
    let credentials =
        |label: &'static str, listed: &mut dyn Iterator<Item = (i64, &String, Option<String>)>| {
            let listed: Vec<Html> = listed
            .map(|(credential, label, title)| html! {
                <span class="cert" {title}>
                    { label }
                    <button class="small" onclick={actions.revoke.reform(move |_| credential)}>{ "Revoke" }</button>
                </span>
            })
            .collect();
            html! { <td class="certs" data-label={label}>if listed.is_empty() { { "-" } }{ for listed }</td> }
        };
    html! {
        <table class="users">
            <thead>
                <tr><th>{ "User" }</th><th>{ "Role" }</th><th>{ "Password" }</th><th>{ "Certificate" }</th><th>{ "Passkey" }</th><th>{ "API Tokens" }</th><th></th></tr>
            </thead>
            <tbody>
                { for users.iter().map(|u| {
                    let id = u.id;
                    let onchange = actions.change.reform(move |e: yew::Event| {
                        let value = e.target_unchecked_into::<web_sys::HtmlSelectElement>().value();
                        (id, Role::parse(&value).unwrap_or(Role::None))
                    });
                    let roles = [Role::None, Role::Read, Role::Write, Role::Admin];
                    let buttons = (!u.builtin).then(|| html! {
                        <>
                            <button class="small" onclick={actions.password.reform(move |_| id)}>{ "Set Password" }</button>
                            <button class="small" onclick={actions.token.reform(move |_| id)}>{ "New Token" }</button>
                            if u.certs.len() < MOST_CERTIFICATES {
                                <button class="small" onclick={actions.certificate.reform(move |_| id)}>{ "New Certificate" }</button>
                            }
                            if u.keys.len() < MOST_PASSKEYS {
                                <button class="small" onclick={actions.passkey.reform(move |_| id)}>{ "New Passkey" }</button>
                            }
                            <button class="small" onclick={actions.remove.reform(move |_| id)}>{ "Remove" }</button>
                        </>
                    });
                    html! {
                        <tr class={classes!(u.builtin.then_some("builtin"))}>
                            <td class="name">{ &u.name }</td>
                            <td data-label="role">
                                <select {onchange}>
                                    { for roles.iter().map(|r| html! {
                                        <option value={r.as_str()} selected={*r == u.role}>{ r.as_str() }</option>
                                    }) }
                                </select>
                            </td>
                            if u.builtin {
                                <td colspan="4" class="hint">{ "Anyone who has not logged in" }</td>
                            } else {
                                <td data-label="password">{ if u.passwords > 0 { "set" } else { "-" } }</td>
                                { credentials("certificate", &mut u.certs.iter().map(|c| (c.id, &c.label, Some(format!("expires {}", c.expires))))) }
                                { credentials("passkey", &mut u.keys.iter().map(|k| (k.id, &k.label, None))) }
                                <td data-label="api tokens">{ match u.tokens { 0 => "-".to_string(), n => n.to_string() } }</td>
                            }
                            <td class="actions">{ for buttons }</td>
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
            <p class="empty">{ "No workers yet. A pipedef that names one adds it, or New Worker does." }</p>
        };
    }
    html! {
        <table class="workers">
            <thead>
                <tr><th>{ "worker" }</th><th>{ "host" }</th><th>{ "arch" }</th><th>{ "labels" }</th><th>{ "version" }</th><th>{ "busy" }</th><th>{ "for" }</th><th>{ "running" }</th><th></th></tr>
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
                <td data-label="host">{ &info.hostname }</td>
                <td data-label="arch">{ info.arch.as_str() }</td>
                <td data-label="labels">{ info.labels.join(", ") }</td>
                <td data-label="version">{ &info.version }</td>
                <td class="busy" data-label="busy">{ format!("{}/{}", worker.tasks.len(), info.capacity) }</td>
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
                { for worker.tasks.iter().map(|t| html! {
                    <a href={format!("/task/{}", t.id)}>
                        { format!("{} {} {}.{}", t.pipeline, short_id(t.run), t.stage, t.task) }
                    </a>
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
