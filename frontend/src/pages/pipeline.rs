// The pipeline page: the run grid, newest run on top, with New Run and the pipeline tabs. With
// ?run=<id>, what the tasks of that run came to instead of the grid, under the pipedef's views.

use crate::{PageProps, account, cancel_run, fetch, follow, send, view};
use bettertest_shared::*;
use gloo::{events::EventListener, utils::window};
use gloo_net::http::Request;
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlElement};
use yew::prelude::*;

pub struct PipelinePage {
    snapshot: Option<PipelineSnapshot>,
    notice: Option<String>,
    // The views of the run whose results show, once the boss has drawn them.
    views: Vec<view::View>,
    // Measured after each render, from the columns and names as the browser lays them out.
    layout: view::GridLayout,
    // A live event moves no column, so only a render after anything else measures again.
    measure: bool,
    main: NodeRef,
    // Made once: a new callback would differ from the old one, and render every row again.
    cancel: Callback<String>,
    // The run whose results show instead of the grid.
    run: Option<String>,
    _resize: EventListener,
    _fonts: Option<EventListener>,
    _stream: crate::Followed,
}

pub enum PipelineMsg {
    Live(Live<PipelineSnapshot>),
    Notice(String),
    Views(Vec<view::View>),
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
        // The stream stops when the page is destroyed, which leaving it for another page does too.
        let stream = follow(
            &url,
            link.callback(PipelineMsg::Live),
            link.callback(PipelineMsg::Notice),
        );
        // Names are measured in the embedded font, which may arrive after the first render.
        let fonts_loaded = link.callback(|()| PipelineMsg::Resized);
        let fonts = gloo::utils::document().fonts();
        let run = (window().location().search().ok())
            .and_then(|query| Some(query.strip_prefix("?run=")?.to_string()));
        if let Some(run) = &run {
            draw_views(ctx, run);
        }
        Self {
            snapshot: None,
            notice: None,
            views: vec![],
            layout: view::GridLayout::default(),
            measure: true,
            main: NodeRef::default(),
            cancel: link.callback(PipelineMsg::Cancel),
            run,
            _resize: EventListener::new(&window(), "resize", move |_| resized.emit(())),
            _fonts: Some(EventListener::new(&fonts, "loadingdone", move |_| {
                fonts_loaded.emit(())
            })),
            _stream: stream,
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
                // Views drawn while the run ran showed it unfinished.
                if let (bettertest_shared::Event::RunFinished { run, .. }, Some(shown)) =
                    (&event, &self.run)
                    && run.to_string() == *shown
                {
                    draw_views(ctx, shown);
                }
                return true;
            }
            PipelineMsg::Notice(text) => self.notice = Some(text),
            PipelineMsg::Views(views) => self.views = views,
            PipelineMsg::Layout(layout) => self.layout = layout,
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
                    if let Err(e) = send(Request::post(&url).build()).await {
                        link.send_message(PipelineMsg::Notice(format!(
                            "could not start a run: {e}"
                        )));
                    }
                });
                return false;
            }
        }
        self.measure = true;
        true
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let (Some(main), Some(snapshot)) = (self.main.cast::<Element>(), &self.snapshot) else {
            return;
        };
        let Ok(pipeline) = &snapshot.pipeline else {
            return;
        };
        if !std::mem::take(&mut self.measure) || self.run.is_some() {
            return;
        }
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
        let mut crumbs = html! {};
        let body = match (&self.snapshot, &self.run) {
            (None, _) => html! {},
            (Some(snapshot), Some(id)) => {
                match snapshot.runs.iter().find(|run| run.id.to_string() == *id) {
                    Some(run) => {
                        crumbs = view::run_page_crumbs(run, &self.cancel);
                        let figure = |figure: &serde_json::Value| {
                            html! { <Figure figure={figure.to_string()} /> }
                        };
                        view::results(run, view::views(&self.views, figure))
                    }
                    None => view::not_recent(id),
                }
            }
            (Some(snapshot), None) => match &snapshot.pipeline {
                Ok(pipeline) => {
                    let rows = snapshot.runs.iter().map(|run| {
                        html! {
                            <RunRow key={run.id.to_string()} pipeline={pipeline.clone()} run={run.clone()}
                                layout={self.layout.clone()} cancel={self.cancel.clone()} />
                        }
                    });
                    view::grid(pipeline, &self.layout, html! { { for rows } })
                }
                Err(e) => view::notice(&Some(format!("the pipedef does not parse:\n{e}"))),
            },
        };
        html! {
            <>
                { view::header(crumbs, html! { <>{ start }{ account() }</> }) }
                { view::notice(&self.notice) }
                <main ref={self.main.clone()}>{ body }</main>
                { tabs }
            </>
        }
    }
}

/// Has the boss draw the views of `run` for its results.
fn draw_views(ctx: &Context<PipelinePage>, run: &str) {
    let url = format!("/api/run/{run}/views");
    let link = ctx.link().clone();
    spawn_local(async move {
        match fetch(Request::get(&url).build()).await {
            Ok(views) => link.send_message(PipelineMsg::Views(views)),
            Err(e) => link.send_message(PipelineMsg::Notice(format!("no views: {e}"))),
        }
    });
}

// Plotly is big, so only a page with a figure on it loads it, once. It draws in the page's own
// colors and type, which CSS variables hold for either color scheme.
#[wasm_bindgen(inline_js = r##"
let plotly;
export function plot(element, figure) {
    plotly ??= new Promise((loaded, failed) => {
        const script = document.createElement("script");
        script.src = "/assets/plotly.v1.min.js";
        script.onload = loaded;
        script.onerror = () => failed("could not load /assets/plotly.v1.min.js");
        document.head.append(script);
    });
    plotly.then(() => {
        const css = getComputedStyle(element);
        const color = (name) => css.getPropertyValue(name).trim();
        const text = { family: css.fontFamily, color: color("--text"), size: 14 };
        const mono = { family: color("--mono"), color: color("--text"), size: 11 };
        const clear = "rgba(0,0,0,0)";
        const axis = { gridcolor: color("--line"), linecolor: color("--line"), zerolinecolor: color("--line"), automargin: true };
        const template = {
            layout: {
                font: text,
                paper_bgcolor: clear,
                plot_bgcolor: clear,
                // Not pass green or fail red: a series in either would read as an outcome.
                colorway: [color("--highlight"), "#ffb700", color("--muted"), color("--text")],
                margin: { l: 16, r: 16, t: 16, b: 16 },
                xaxis: axis,
                yaxis: axis,
                hoverlabel: { bgcolor: color("--surface"), bordercolor: color("--line"), font: text },
            },
            data: {
                pie: [{ marker: { line: { color: color("--background"), width: 2 } } }],
                table: [{
                    header: { fill: { color: color("--surface") }, line: { color: color("--line") }, font: { ...mono, color: color("--muted") }, align: "left" },
                    cells: { fill: { color: clear }, line: { color: color("--line") }, font: mono, align: "left" },
                }],
            },
        };
        const { data, layout } = JSON.parse(figure);
        const config = { responsive: true, displayModeBar: false };
        Plotly.react(element, data, { ...layout, template }, config);
    }, (problem) => { element.textContent = problem; });
}
export function purge(element) {
    window.Plotly?.purge(element);
}
"##)]
extern "C" {
    fn plot(element: &Element, figure: &str);
    fn purge(element: &Element);
}

/// A Plotly figure, drawn by plotly.js into a div of its own.
struct Figure {
    node: NodeRef,
}

#[derive(Properties, PartialEq)]
struct FigureProps {
    // The figure as JSON: its data and layout.
    figure: String,
}

impl Component for Figure {
    type Message = ();
    type Properties = FigureProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            node: NodeRef::default(),
        }
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        html! { <div class="figure" ref={self.node.clone()}></div> }
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        if let Some(element) = self.node.cast::<Element>() {
            plot(&element, &ctx.props().figure);
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(element) = self.node.cast::<Element>() {
            purge(&element);
        }
    }
}

/// One run's row of the grid. A live event changes one run, so only its row renders again: with
/// thousands of tasks, rendering every row for every event kept the browser busy.
struct RunRow;

#[derive(Properties)]
struct RunRowProps {
    pipeline: Arc<Pipeline>,
    run: Arc<Run>,
    layout: view::GridLayout,
    cancel: Callback<String>,
}

impl PartialEq for RunRowProps {
    fn eq(&self, other: &Self) -> bool {
        // The snapshot replaces a run that changes, so the pointer says whether it did.
        Arc::ptr_eq(&self.pipeline, &other.pipeline)
            && Arc::ptr_eq(&self.run, &other.run)
            && self.layout == other.layout
            && self.cancel == other.cancel
    }
}

impl Component for RunRow {
    type Message = ();
    type Properties = RunRowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        view::run_row(&props.pipeline, &props.run, &props.layout, &props.cancel)
    }
}
