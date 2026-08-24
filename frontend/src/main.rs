use bettertest_shared_crate::*;
use gloo::{console::*, net::http::*};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::*;
use web_sys::{EventSource, MessageEvent};
use yew::prelude::*;

struct PipelineView {
    pipeline: Option<Pipeline>,
    runs: Vec<Run>,
    source: EventSource,
    listeners: Vec<Closure<dyn FnMut(MessageEvent)>>,
}

enum Msg {
    Loaded(StateResponse),
    StartRun,
    Run(Run),
}

fn cols(pipeline: &Pipeline) -> String {
    pipeline
        .stages
        .iter()
        .map(|s| format!("{}fr", 1.0 + s.tasks.len() as f32 / 2.0))
        .collect::<Vec<_>>()
        .join(" ")
}

fn task_html(pipeline_task: &Task, run: &Run, stage_name: &str) -> Html {
    let Some(task) = run
        .stages
        .iter()
        .find(|s| s.name == stage_name)
        .and_then(|s| s.tasks.iter().find(|t| t.name == pipeline_task.name))
    else {
        return html! { <div class="cell pending"></div> };
    };
    let class = match task.state {
        TaskState::Pending => "cell pending",
        TaskState::Running => "cell running",
        TaskState::Pass => "cell pass",
        TaskState::Fail => "cell fail",
    };
    html! {
        <a class={class} href={format!("/logs?task={}", task.id)}></a>
    }
}

fn stage_html(stage: &Stage, run: &Run) -> Html {
    let n = stage.tasks.len();
    html! {
        <div style={format!("display:grid; column-gap:2px; grid-template-columns: repeat({n}, 1fr);")}>
            { for stage.tasks.iter().map(|task| task_html(task, run, &stage.name)) }
        </div>
    }
}

fn run_html(pipeline: &Pipeline, run: &Run) -> Html {
    html! {
        <div class="run-row">
            <div class="run-number">{ run.number }</div>
            <div style={format!("display:grid; column-gap:8px; grid-template-columns: {}; flex:1;", cols(pipeline))}>
                { for pipeline.stages.iter().map(|stage| stage_html(stage, run)) }
            </div>
        </div>
    }
}

fn pipeline_header(pipeline: &Pipeline) -> Html {
    html! {
        <div class="run-row">
            <div class="run-number"></div>
            <div style={format!("display:grid; column-gap:8px; grid-template-columns: {}; flex:1;", cols(pipeline))}>
                { for pipeline.stages.iter().map(|stage| html! {
                    <div class="stage-name">{ &stage.name }</div>
                }) }
            </div>
        </div>
    }
}

impl Component for PipelineView {
    type Message = Msg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link().clone();
        spawn_local(async move {
            match Request::get("/api/state").send().await {
                Ok(response) if response.ok() => {
                    if let Ok(state) = response.json::<StateResponse>().await {
                        link.send_message(Msg::Loaded(state));
                    }
                }
                Ok(response) => error!(format!("HTTP {}", response.status())),
                Err(e) => error!(e.to_string()),
            }
        });
        let source = EventSource::new("/api/events").unwrap();
        let link = ctx.link().clone();
        let on_run = Closure::wrap(Box::new(move |e: MessageEvent| {
            let run = serde_json::from_str(&e.data().as_string().unwrap()).unwrap();
            link.send_message(Msg::Run(run));
        }) as Box<dyn FnMut(MessageEvent)>);
        source
            .add_event_listener_with_callback("run", on_run.as_ref().unchecked_ref())
            .unwrap();
        Self {
            pipeline: None,
            runs: vec![],
            source,
            listeners: vec![on_run],
        }
    }

    fn update(&mut self, _: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Loaded(state) => {
                self.pipeline = Some(state.pipeline);
                self.runs = state.runs;
                true
            }
            Msg::StartRun => {
                spawn_local(async {
                    let _ = Request::post("/api/run").send().await;
                });
                false
            }
            Msg::Run(run) => {
                if let Some(existing) = self.runs.iter_mut().find(|r| r.id == run.id) {
                    *existing = run;
                } else {
                    self.runs.insert(0, run);
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let Some(pipeline) = &self.pipeline else {
            return html! {};
        };
        html! {
            <div>
                <button onclick={ctx.link().callback(|_| Msg::StartRun)}>
                    { "new run" }
                </button>
                <div class="grid">
                    { pipeline_header(pipeline) }
                    { for self.runs.iter().map(|run| run_html(pipeline, run)) }
                </div>
            </div>
        }
    }

    fn destroy(&mut self, _: &Context<Self>) {
        self.source.close();
        self.listeners.clear();
    }
}

struct LogsPage {
    stage: String,
    name: String,
    output: String,
    source: Option<EventSource>,
    listeners: Vec<Closure<dyn FnMut(MessageEvent)>>,
}

enum LogsMsg {
    Snapshot(TaskLog),
    Line(String),
}

fn query_param(key: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let s = search.strip_prefix('?').unwrap_or(&search);
    s.split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

impl Component for LogsPage {
    type Message = LogsMsg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let task_id = query_param("task");
        let mut source = None;
        let mut listeners = Vec::new();
        if let Some(id) = task_id.clone() {
            let es = EventSource::new("/api/events").unwrap();
            let link = ctx.link().clone();
            let on_log = Closure::wrap(Box::new(move |e: MessageEvent| {
                let line: LogLine = serde_json::from_str(&e.data().as_string().unwrap()).unwrap();
                if line.task_id.to_string() == id {
                    link.send_message(LogsMsg::Line(line.line));
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            es.add_event_listener_with_callback("log", on_log.as_ref().unchecked_ref())
                .unwrap();
            listeners.push(on_log);
            source = Some(es);
        }
        if let Some(id) = task_id {
            let link = ctx.link().clone();
            spawn_local(async move {
                let url = format!("/api/logs/{id}");
                if let Ok(resp) = Request::get(&url).send().await
                    && let Ok(log) = resp.json::<TaskLog>().await
                {
                    link.send_message(LogsMsg::Snapshot(log));
                }
            });
        }
        Self {
            stage: String::new(),
            name: String::new(),
            output: String::new(),
            source,
            listeners,
        }
    }

    fn update(&mut self, _: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            LogsMsg::Snapshot(log) => {
                self.stage = log.stage;
                self.name = log.name;
                self.output = log.output;
                true
            }
            LogsMsg::Line(line) => {
                if !self.output.is_empty() {
                    self.output.push('\n');
                }
                self.output.push_str(&line);
                true
            }
        }
    }

    fn view(&self, _: &Context<Self>) -> Html {
        html! {
            <div class="logs-page">
                <header>
                    <h1>
                        <a href="/">{ "bettertest" }</a>
                        { " > " }
                        { &self.stage }
                        { " > " }
                        { &self.name }
                    </h1>
                </header>
                <pre class="logs">{ &self.output }</pre>
            </div>
        }
    }

    fn destroy(&mut self, _: &Context<Self>) {
        if let Some(source) = &self.source {
            source.close();
        }
        self.listeners.clear();
    }
}

fn main() {
    let path = web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default();
    if path == "/logs" {
        yew::Renderer::<LogsPage>::new().render();
    } else {
        yew::Renderer::<PipelineView>::new().render();
    }
}
