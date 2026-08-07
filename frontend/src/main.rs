use gloo::console::log;

use bettertest_shared_crate::*;
use gloo::{console::error, net::http::Request};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

struct PipelineGrid {
    pipeline: Option<PipelineResponse>,
}

fn task_html(task: &Task) -> Html {
    html! {
        <div style="height:100px;background:blue;margin-bottom:10px">{task.name.to_string()}</div>
    }
}

fn stage_html(stage: &Stage) -> Html {
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
            "display:grid; column-gap:8px; grid-template-columns: {weights_str};"
        )}>
            { for stage.tasks.iter().map(task_html) }
        </div>
    }
}

fn run_html(run: &Run) -> Html {
    let mut weights = Vec::new();
    for stage in &run.stages {
        weights.push(1.0 + stage.tasks.len() as f32 / 2.0);
    }

    let weights_str = weights
        .into_iter()
        .map(|w| format!("{w}fr"))
        .collect::<Vec<_>>()
        .join(" ");

    html! {
        <div style={format!(
            "display:grid; column-gap:8px; grid-template-columns: {weights_str};"
        )}>
            { for run.stages.iter().map(stage_html) }
        </div>
    }
}

impl Component for PipelineGrid {
    type Message = PipelineResponse;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        log!("here in update");

        let link = ctx.link().clone();
        spawn_local(async move {
            let result = async {
                let response = Request::get("/api/pipeline")
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;

                if !response.ok() {
                    return Err(format!("HTTP {}", response.status()));
                }

                response
                    .json::<PipelineResponse>()
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;

            match result {
                Ok(pipeline) => link.send_message(pipeline),
                Err(e) => error!(e),
            }
        });
        Self { pipeline: None }
    }

    fn update(&mut self, _: &Context<Self>, msg: Self::Message) -> bool {
        log!("here in update");

        self.pipeline = Some(msg);
        true
    }

    fn view(&self, _: &Context<Self>) -> Html {
        let Some(pipeline) = &self.pipeline else {
            return html! {};
        };
        html! {
            <div style="width:100%;">
                { for pipeline.runs.iter().map(run_html) }
            </div>
        }
    }
}

fn main() {
    yew::Renderer::<PipelineGrid>::new().render();
}
