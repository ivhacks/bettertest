// The pipeline page: the run grid, newest run on top, with New Run and the pipeline tabs.

use crate::{PageProps, account, cancel_run, follow, view};
use bettertest_shared::*;
use gloo::{events::EventListener, utils::window};
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlElement, wasm_bindgen::JsCast};
use yew::prelude::*;

pub struct PipelinePage {
    snapshot: Option<PipelineSnapshot>,
    notice: Option<String>,
    // Measured after each render, from the columns and names as the browser lays them out.
    layout: view::GridLayout,
    main: NodeRef,
    _resize: EventListener,
    _fonts: Option<EventListener>,
}

pub enum PipelineMsg {
    Live(Live<PipelineSnapshot>),
    Notice(String),
    StartRun,
    Cancel(String),
    Layout(view::GridLayout),
    Resized,
}

impl Component for PipelinePage {
    type Message = PipelineMsg;
    type Properties = PageProps;

    fn create(ctx: &Context<Self>) -> Self {
        let url = format!("/api/pipeline/{}/events", ctx.props().id);
        let link = ctx.link();
        let resized = link.callback(|()| PipelineMsg::Resized);
        // Every page is a full page load, so the stream lives exactly as long as the component.
        follow(
            &url,
            link.callback(PipelineMsg::Live),
            link.callback(PipelineMsg::Notice),
        );
        // Names are measured in the embedded font, which may arrive after the first render.
        let fonts_loaded = link.callback(|()| PipelineMsg::Resized);
        let fonts = gloo::utils::document().fonts();
        Self {
            snapshot: None,
            notice: None,
            layout: view::GridLayout::default(),
            main: NodeRef::default(),
            _resize: EventListener::new(&window(), "resize", move |_| resized.emit(())),
            _fonts: Some(EventListener::new(&fonts, "loadingdone", move |_| {
                fonts_loaded.emit(())
            })),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            PipelineMsg::Live(Live::Snapshot(snapshot)) => {
                self.snapshot = Some(snapshot);
                self.notice = None;
            }
            PipelineMsg::Live(Live::Event(event)) => {
                if let Some(snapshot) = &mut self.snapshot {
                    snapshot.apply(&event);
                }
            }
            PipelineMsg::Notice(text) => self.notice = Some(text),
            PipelineMsg::Layout(layout) => self.layout = layout,
            // Rendering again measures again.
            PipelineMsg::Resized => {}
            PipelineMsg::Cancel(run) => {
                cancel_run(run, ctx.link().callback(PipelineMsg::Notice));
                return false;
            }
            PipelineMsg::StartRun => {
                let url = format!("/api/start-run/{}", ctx.props().id);
                let link = ctx.link().clone();
                // The new run shows up through the live stream like any other change.
                spawn_local(async move {
                    match Request::post(&url).send().await {
                        Ok(response) if response.ok() => {}
                        Ok(response) => {
                            let body = response.text().await.unwrap_or_default();
                            link.send_message(PipelineMsg::Notice(format!(
                                "could not start a run: {body}"
                            )));
                        }
                        Err(e) => link.send_message(PipelineMsg::Notice(format!(
                            "could not start a run: {e}"
                        ))),
                    }
                });
                return false;
            }
        }
        true
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let (Some(main), Some(snapshot)) = (self.main.cast::<Element>(), &self.snapshot) else {
            return;
        };
        let Ok(pipeline) = &snapshot.pipeline else {
            return;
        };
        let elements = |selector: &str| {
            let mut found = vec![];
            if let Ok(list) = main.query_selector_all(selector) {
                for i in 0..list.length() {
                    if let Some(element) =
                        list.item(i).and_then(|n| n.dyn_into::<HtmlElement>().ok())
                    {
                        found.push(element);
                    }
                }
            }
            found
        };
        // Rotation leaves offset sizes alone, so these are the flat sizes whatever the angle.
        let columns: Vec<f64> = elements(".grid th.task")
            .iter()
            .map(|e| e.client_width() as f64)
            .collect();
        let names = elements(".grid th.task span");
        let labels: Vec<f64> = names.iter().map(|e| e.offset_width() as f64).collect();
        let line = names.first().map_or(0.0, |e| e.offset_height() as f64);
        // A stage whose columns are narrower than a line cannot show its tasks' names, even
        // upright: its tasks run together and leave their names to the tooltips.
        let mut dense = vec![];
        let mut shown = (vec![], vec![]);
        let mut at = 0;
        for stage in &pipeline.stages {
            let width = columns.get(at).copied().unwrap_or(f64::INFINITY);
            let packed = width < line;
            dense.push(packed);
            if !packed {
                let end = (at + stage.tasks.len()).min(columns.len());
                shown.0.extend(columns.get(at..end).unwrap_or_default());
                shown.1.extend(labels.get(at..end).unwrap_or_default());
            }
            at += stage.tasks.len();
        }
        let (angle, height) = view::label_layout(&shown.0, &shown.1, line);
        let layout = view::GridLayout {
            angle,
            height,
            dense,
        };
        // Only a real change renders again, so this settles after one round.
        let moved = (layout.angle - self.layout.angle).abs() > 0.5
            || (layout.height - self.layout.height).abs() > 1.0
            || layout.dense != self.layout.dense;
        if moved {
            ctx.link().send_message(PipelineMsg::Layout(layout));
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let start = html! {
            <button class="primary" onclick={ctx.link().callback(|_| PipelineMsg::StartRun)}>{ "New Run" }</button>
        };
        let tabs = match &self.snapshot {
            Some(snapshot) => view::tabs(&snapshot.pipelines, &snapshot.name),
            None => html! {},
        };
        let body = match &self.snapshot {
            Some(PipelineSnapshot {
                pipeline: Ok(pipeline),
                runs,
                ..
            }) => view::grid(
                pipeline,
                runs,
                &self.layout,
                &ctx.link().callback(PipelineMsg::Cancel),
            ),
            Some(PipelineSnapshot {
                pipeline: Err(e), ..
            }) => view::notice(&Some(format!("the pipedef does not parse:\n{e}"))),
            None => html! {},
        };
        html! {
            <>
                { view::header(html! {}, html! { <>{ start }{ account() }</> }) }
                { view::notice(&self.notice) }
                <main ref={self.main.clone()}>{ body }</main>
                { tabs }
            </>
        }
    }
}
