use bettertest_shared_crate::*;
use gloo::{console::*, net::http::*};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::*;
use web_sys::{EventSource, MessageEvent};
use yew::prelude::*;

struct Live {
    _source: EventSource,
    _on_run: Closure<dyn FnMut(MessageEvent)>,
    _on_task: Closure<dyn FnMut(MessageEvent)>,
    _on_done: Closure<dyn FnMut(MessageEvent)>,
}

struct PipelineView {
    pipeline: Option<Pipeline>,
    runs: Vec<Run>,
    _live: Option<Live>,
}

enum Msg {
    Loaded(StateResponse),
    StartRun,
    RunStarted(Run),
    Task(TaskUpdate),
    RunDone(RunDone),
}

fn connect(link: yew::html::Scope<PipelineView>) -> Live {
    let source = EventSource::new("/api/events").unwrap();

    let on_run = {
        let link = link.clone();
        Closure::wrap(Box::new(move |e: MessageEvent| {
            let data = e.data().as_string().unwrap();
            let run = serde_json::from_str(&data).unwrap();
            link.send_message(Msg::RunStarted(run));
        }) as Box<dyn FnMut(MessageEvent)>)
    };
    source
        .add_event_listener_with_callback("run_started", on_run.as_ref().unchecked_ref())
        .unwrap();

    let on_task = {
        let link = link.clone();
        Closure::wrap(Box::new(move |e: MessageEvent| {
            let data = e.data().as_string().unwrap();
            let update = serde_json::from_str(&data).unwrap();
            link.send_message(Msg::Task(update));
        }) as Box<dyn FnMut(MessageEvent)>)
    };
    source
        .add_event_listener_with_callback("task", on_task.as_ref().unchecked_ref())
        .unwrap();

    let on_done = {
        let link = link.clone();
        Closure::wrap(Box::new(move |e: MessageEvent| {
            let data = e.data().as_string().unwrap();
            let done = serde_json::from_str(&data).unwrap();
            link.send_message(Msg::RunDone(done));
        }) as Box<dyn FnMut(MessageEvent)>)
    };
    source
        .add_event_listener_with_callback("run_done", on_done.as_ref().unchecked_ref())
        .unwrap();

    Live {
        _source: source,
        _on_run: on_run,
        _on_task: on_task,
        _on_done: on_done,
    }
}

fn cell_class(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "cell pending",
        TaskState::Running => "cell running",
        TaskState::Pass => "cell pass",
        TaskState::Fail => "cell fail",
    }
}

fn task_html(pipeline_task: &Task, run: &Run, stage_name: &str) -> Html {
    let state = run
        .stages
        .iter()
        .find(|s| s.name == stage_name)
        .and_then(|s| s.tasks.iter().find(|t| t.name == pipeline_task.name))
        .map(|t| t.state)
        .unwrap_or(TaskState::Pending);
    html! { <div class={cell_class(state)}></div> }
}

fn stage_html(stage: &Stage, run: &Run) -> Html {
    let mut weights = Vec::new();
    for _task in &stage.tasks {
        weights.push(1);
    }
    let weights_str = weights
        .into_iter()
        .map(|w| format!("{w}fr"))
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        <div style={format!(
            "display:grid; column-gap:2px; grid-template-columns: {weights_str};"
        )}>
            { for stage.tasks.iter().map(|task| task_html(task, run, &stage.name)) }
        </div>
    }
}

fn run_html(pipeline: &Pipeline, run: &Run) -> Html {
    let mut weights = Vec::new();
    for stage in &pipeline.stages {
        weights.push(1.0 + stage.tasks.len() as f32 / 2.0);
    }
    let weights_str = weights
        .into_iter()
        .map(|w| format!("{w}fr"))
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        <div class="run-row">
            <div class="run-number">{ run.number }</div>
            <div style={format!(
                "display:grid; column-gap:8px; grid-template-columns: {weights_str}; flex:1;"
            )}>
                { for pipeline.stages.iter().map(|stage| stage_html(stage, run)) }
            </div>
        </div>
    }
}

fn pipeline_header(pipeline: &Pipeline) -> Html {
    let mut weights = Vec::new();
    for stage in &pipeline.stages {
        weights.push(1.0 + stage.tasks.len() as f32 / 2.0);
    }
    let weights_str = weights
        .into_iter()
        .map(|w| format!("{w}fr"))
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        <div class="run-row">
            <div class="run-number"></div>
            <div style={format!(
                "display:grid; column-gap:8px; grid-template-columns: {weights_str}; flex:1;"
            )}>
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
            let result = async {
                let response = Request::get("/api/state")
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !response.ok() {
                    return Err(format!("HTTP {}", response.status()));
                }
                response
                    .json::<StateResponse>()
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            match result {
                Ok(state) => link.send_message(Msg::Loaded(state)),
                Err(e) => error!(e),
            }
        });
        Self {
            pipeline: None,
            runs: vec![],
            _live: Some(connect(ctx.link().clone())),
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
            Msg::RunStarted(run) => {
                if !self.runs.iter().any(|r| r.id == run.id) {
                    self.runs.insert(0, run);
                }
                true
            }
            Msg::Task(update) => {
                if let Some(task) = self
                    .runs
                    .iter_mut()
                    .find(|r| r.id == update.run_id)
                    .and_then(|r| {
                        r.stages
                            .iter_mut()
                            .flat_map(|s| s.tasks.iter_mut())
                            .find(|t| t.id == update.task_id)
                    })
                {
                    task.state = update.state;
                }
                true
            }
            Msg::RunDone(done) => {
                if let Some(run) = self.runs.iter_mut().find(|r| r.id == done.run_id) {
                    run.active = false;
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
}

fn main() {
    yew::Renderer::<PipelineView>::new().render();
}
