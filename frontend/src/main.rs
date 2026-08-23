use bettertest_shared_crate::*;
use gloo::{console::*, net::http::*};
use wasm_bindgen_futures::*;
use yew::prelude::*;

struct PipelineView {
    pipeline: Option<Pipeline>,
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

fn pipeline_html(pipeline: &Pipeline) -> Html {
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
        <div style={format!(
            "display:grid; column-gap:8px; grid-template-columns: {weights_str};"
        )}>
            { for pipeline.stages.iter().map(stage_html) }
        </div>
    }
}

impl Component for PipelineView {
    type Message = Pipeline;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
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

                response.json::<Pipeline>().await.map_err(|e| e.to_string())
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
        self.pipeline = Some(msg);
        true
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let Some(pipeline) = &self.pipeline else {
            return html! {};
        };
        html! {
            <div>
                // <button onclick={ctx.link().callback(|_| Msg::StartRun)}>
                //     { "new run" }
                // </button>
                <div style="width:100%;">
                    { pipeline_html(pipeline) }
                </div>
            </div>
        }
    }
}

fn main() {
    yew::Renderer::<PipelineView>::new().render();
}
