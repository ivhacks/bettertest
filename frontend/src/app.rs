pub use bettertest_common::*;
use gloo_net::http::Request;
use wasm_bindgen::prelude::*;
use web_sys::EventSource;
use yew::prelude::*;

#[derive(Clone, PartialEq, Debug)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn class(&self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn toggle_label(&self) -> &'static str {
        match self {
            Theme::Dark => "light",
            Theme::Light => "dark",
        }
    }

    pub fn toggled(&self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

// --- messages ---

pub enum Msg {
    ToggleTheme,
    StartRun,
    StateLoaded(StateResponse),
    RunCreated {
        run_id: u32,
    },
    SseState(PipelineRunState),
    StageStarted {
        stage_name: String,
    },
    TaskResult {
        stage_name: String,
        task_name: String,
        passed: bool,
    },
    RunDone,
}

// --- update (pure logic, separated for testing) ---

pub struct UpdateResult {
    pub changed: bool,
    pub start_run: bool,
    pub open_sse: Option<u32>,
    pub close_sse: bool,
}

pub fn update_state(
    pipeline: &Option<PipelineDto>,
    run: &mut Option<PipelineRunState>,
    theme: &mut Theme,
    msg: Msg,
) -> UpdateResult {
    let mut result = UpdateResult {
        changed: false,
        start_run: false,
        open_sse: None,
        close_sse: false,
    };

    match msg {
        Msg::ToggleTheme => {
            *theme = theme.toggled();
            result.changed = true;
        }
        Msg::StateLoaded(ref s) => {
            // pipeline is set in Component::update
            if let Some(ref snap) = s.run {
                if snap.active {
                    result.open_sse = Some(snap.run_id);
                }
                *run = Some(snap.clone());
            }
            result.changed = true;
        }
        Msg::StartRun => {
            *run = None;
            result.start_run = true;
            result.close_sse = true;
            result.changed = true;
        }
        Msg::RunCreated { run_id } => {
            if pipeline.is_some() {
                result.open_sse = Some(run_id);
            }
            result.changed = true;
        }
        Msg::SseState(state) => {
            *run = Some(state);
            result.changed = true;
        }
        Msg::StageStarted { stage_name } => {
            if let Some(r) = run
                && let Some(stage) = r
                    .stages
                    .iter_mut()
                    .find(|s| s.name == stage_name)
            {
                for task in &mut stage.tasks {
                    task.state = TaskState::Running;
                }
            }
            result.changed = true;
        }
        Msg::TaskResult {
            stage_name,
            task_name,
            passed,
        } => {
            if let Some(r) = run
                && let Some(stage) = r
                    .stages
                    .iter_mut()
                    .find(|s| s.name == stage_name)
                && let Some(task) = stage
                    .tasks
                    .iter_mut()
                    .find(|t| t.name == task_name)
            {
                task.state = if passed {
                    TaskState::Pass
                } else {
                    TaskState::Fail
                };
            }
            result.changed = true;
        }
        Msg::RunDone => {
            result.close_sse = true;
            result.changed = true;
        }
    }

    result
}

// --- view (pure functions, separated for testing) ---

pub fn view_app(
    pipeline: &Option<PipelineDto>,
    run: &Option<PipelineRunState>,
    theme: &Theme,
    link: &yew::html::Scope<App>,
) -> Html {
    html! {
        <div class={theme.class()}>
            <header>
                <h1>{ "bettertest" }</h1>
                <div class="buttons">
                    <button onclick={link.callback(|_| Msg::StartRun)} disabled={pipeline.is_none()}>
                        { "new run" }
                    </button>
                    <button onclick={link.callback(|_| Msg::ToggleTheme)}>
                        { theme.toggle_label() }
                    </button>
                </div>
            </header>
            { view_run(run, theme) }
        </div>
    }
}

pub fn view_run(run: &Option<PipelineRunState>, theme: &Theme) -> Html {
    match run {
        None => html! {},
        Some(r) => html! {
            { for r.stages.iter().map(|s| view_stage(s, r.run_id, theme)) }
        },
    }
}

pub fn view_stage(stage: &StageRunState, run_id: u32, theme: &Theme) -> Html {
    html! {
        <section>
            <h2>{ &stage.name }</h2>
            <ul>
                { for stage.tasks.iter().map(|t| view_task(t, &stage.name, run_id, theme)) }
            </ul>
        </section>
    }
}

pub fn view_task(task: &TaskRunState, stage_name: &str, run_id: u32, theme: &Theme) -> Html {
    let class = match task.state {
        TaskState::Pending => "pending",
        TaskState::Running => "running",
        TaskState::Pass => "pass",
        TaskState::Fail => "fail",
    };
    let href = format!(
        "/logs?run={run_id}&stage={stage_name}&task={}&theme={}",
        task.name,
        theme.class()
    );
    html! {
        <li class={class}><a href={href}>{ &task.name }</a></li>
    }
}

// --- component (wiring only) ---

pub struct App {
    pipeline: Option<PipelineDto>,
    run: Option<PipelineRunState>,
    theme: Theme,
    event_source: Option<EventSource>,
    _closures: Vec<Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

impl App {
    fn close_event_source(&mut self) {
        if let Some(es) = self.event_source.take() {
            es.close();
        }
        self._closures.clear();
    }

    fn open_event_source(&mut self, run_id: u32, link: &yew::html::Scope<Self>) {
        self.close_event_source();

        let url = format!("/api/run/{}/events", run_id);
        let Ok(es) = EventSource::new(&url) else {
            return;
        };

        // state (full snapshot)
        {
            let link = link.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
                if let Some(data) = e.data().as_string()
                    && let Ok(state) = serde_json::from_str::<PipelineRunState>(&data)
                {
                    link.send_message(Msg::SseState(state));
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("state", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        // stage_started
        {
            let link = link.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
                if let Some(data) = e.data().as_string()
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(&data)
                    && let Some(stage) = val["stage"].as_str()
                {
                    link.send_message(Msg::StageStarted {
                        stage_name: stage.to_string(),
                    });
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("stage_started", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        // task_result
        {
            let link = link.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
                if let Some(data) = e.data().as_string()
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(&data)
                    && let Some(stage) = val["stage"].as_str()
                    && let Some(task) = val["task"].as_str()
                {
                    let passed = val["passed"].as_bool().unwrap_or(false);
                    link.send_message(Msg::TaskResult {
                        stage_name: stage.to_string(),
                        task_name: task.to_string(),
                        passed,
                    });
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("task_result", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        // run_done
        {
            let link = link.clone();
            let cb = Closure::wrap(Box::new(move |_e: web_sys::MessageEvent| {
                link.send_message(Msg::RunDone);
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("run_done", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        self.event_source = Some(es);
    }
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link().clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(resp) = Request::get("/api/state").send().await
                && let Ok(state) = resp.json::<StateResponse>().await
            {
                link.send_message(Msg::StateLoaded(state));
            }
        });

        Self {
            pipeline: None,
            run: None,
            theme: Theme::Dark,
            event_source: None,
            _closures: vec![],
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        if let Msg::StateLoaded(ref s) = msg {
            self.pipeline = Some(s.pipeline.clone());
        }

        let result = update_state(&self.pipeline, &mut self.run, &mut self.theme, msg);

        if result.close_sse {
            self.close_event_source();
        }
        if let Some(run_id) = result.open_sse {
            self.open_event_source(run_id, ctx.link());
        }
        if result.start_run {
            let link = ctx.link().clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(resp) = Request::post("/api/run").send().await else {
                    return;
                };
                let Ok(body) = resp.text().await else {
                    return;
                };
                let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) else {
                    return;
                };
                let Some(run_id) = val["run_id"].as_u64() else {
                    return;
                };
                link.send_message(Msg::RunCreated {
                    run_id: run_id as u32,
                });
            });
        }

        result.changed
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        view_app(&self.pipeline, &self.run, &self.theme, ctx.link())
    }
}

// --- logs page ---

pub enum LogsMsg {
    SetOutput(String),
    AppendLine(String),
    ToggleTheme,
}

pub struct LogsPage {
    pub stage: String,
    pub task: String,
    pub theme: Theme,
    pub output: String,
    event_source: Option<EventSource>,
    _closures: Vec<Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

pub fn update_logs(theme: &mut Theme, output: &mut String, msg: LogsMsg) -> bool {
    match msg {
        LogsMsg::SetOutput(text) => {
            *output = text;
            true
        }
        LogsMsg::AppendLine(line) => {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&line);
            true
        }
        LogsMsg::ToggleTheme => {
            *theme = theme.toggled();
            true
        }
    }
}

pub fn view_logs_page(
    stage: &str,
    task: &str,
    theme: &Theme,
    output: &str,
    on_toggle: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class={theme.class()}>
            <header>
                <h1>
                    <a href="/">{"bettertest"}</a>{" > "}{stage}{" > "}{task}
                </h1>
                <div class="buttons">
                    <button onclick={on_toggle}>{ theme.toggle_label() }</button>
                </div>
            </header>
            <pre class="logs">{ output }</pre>
        </div>
    }
}

fn get_query_param(search: &str, key: &str) -> Option<String> {
    let s = search.strip_prefix('?').unwrap_or(search);
    s.split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

fn find_task_output(state: &PipelineRunState, stage: &str, task: &str) -> Option<String> {
    state
        .stages
        .iter()
        .find(|s| s.name == stage)
        .and_then(|s| s.tasks.iter().find(|t| t.name == task))
        .filter(|t| !t.output.is_empty())
        .map(|t| t.output.clone())
}

impl Component for LogsPage {
    type Message = LogsMsg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let search = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();

        let stage = get_query_param(&search, "stage").unwrap_or_default();
        let task = get_query_param(&search, "task").unwrap_or_default();
        let run_id: u32 = get_query_param(&search, "run")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let mut page = Self {
            stage,
            task,
            theme: match get_query_param(&search, "theme").as_deref() {
                Some("light") => Theme::Light,
                _ => Theme::Dark,
            },
            output: String::new(),
            event_source: None,
            _closures: vec![],
        };

        if run_id > 0 {
            page.open_event_source(run_id, ctx.link());
        }

        page
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        update_logs(&mut self.theme, &mut self.output, msg)
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        view_logs_page(
            &self.stage,
            &self.task,
            &self.theme,
            &self.output,
            ctx.link().callback(|_| LogsMsg::ToggleTheme),
        )
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(es) = self.event_source.take() {
            es.close();
        }
    }
}

impl LogsPage {
    fn open_event_source(&mut self, run_id: u32, link: &yew::html::Scope<Self>) {
        let url = format!("/api/run/{}/events", run_id);
        let Ok(es) = EventSource::new(&url) else {
            return;
        };

        // state snapshot — extract output for our task
        {
            let link = link.clone();
            let stage = self.stage.clone();
            let task = self.task.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
                if let Some(data) = e.data().as_string()
                    && let Ok(state) = serde_json::from_str::<PipelineRunState>(&data)
                    && let Some(output) = find_task_output(&state, &stage, &task)
                {
                    link.send_message(LogsMsg::SetOutput(output));
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("state", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        // task_output — streaming log lines
        {
            let link = link.clone();
            let stage = self.stage.clone();
            let task = self.task.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
                if let Some(data) = e.data().as_string()
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(&data)
                    && val["stage"].as_str() == Some(&stage)
                    && val["task"].as_str() == Some(&task)
                    && let Some(line) = val["line"].as_str()
                {
                    link.send_message(LogsMsg::AppendLine(line.to_string()));
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("task_output", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        // task_result — get output when our task finishes
        {
            let link = link.clone();
            let stage = self.stage.clone();
            let task = self.task.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
                if let Some(data) = e.data().as_string()
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(&data)
                    && val["stage"].as_str() == Some(&stage)
                    && val["task"].as_str() == Some(&task)
                    && let Some(output) = val["output"].as_str()
                {
                    link.send_message(LogsMsg::SetOutput(output.to_string()));
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("task_result", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        // run_done — close the stream
        {
            let es_clone = es.clone();
            let cb = Closure::wrap(Box::new(move |_e: web_sys::MessageEvent| {
                es_clone.close();
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            es.add_event_listener_with_callback("run_done", cb.as_ref().unchecked_ref())
                .ok();
            self._closures.push(cb);
        }

        self.event_source = Some(es);
    }
}
