use bettertest_shared_crate::PipelineResponse;
use gloo::{console::error, net::http::Request};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

struct PipelineGrid {
    pipeline: Option<PipelineResponse>,
}

impl Component for PipelineGrid {
    type Message = PipelineResponse;
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
        self.pipeline = Some(msg);
        true
    }

    fn view(&self, _: &Context<Self>) -> Html {
        let Some(pipeline) = &self.pipeline else {
            return html! {};
        };
        let json = serde_json::to_string_pretty(pipeline).unwrap();
        html! { <pre>{ json }</pre> }
    }
}

fn main() {
    yew::Renderer::<PipelineGrid>::new().render();
}


