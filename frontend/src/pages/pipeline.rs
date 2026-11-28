// The pipeline page: the run grid, newest run on top, with New Run and the pipeline tabs. With
// ?run=<id>, what the tasks of that run came to instead of the grid, under the pipedef's views.
// One page stays mounted across pipelines and holds them all, kept live: it opens on the one the
// URL names, then loads every pipeline in one stream behind it, so a tab paints from memory. A
// pipeline shown for the first time is measured off-screen while the grid that is up stays:
// painting it before its columns were measured would paint the header alone.

use crate::{account, act, cancel_run, follow, load, view};
use bettertest_shared::*;
use gloo::{events::EventListener, utils::window};
use gloo_net::http::Request;
use std::{collections::HashMap, sync::Arc};
use wasm_bindgen::prelude::*;
use web_sys::{Element, HtmlElement};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct PipelineProps {
    pub id: String,
    // `?run=`, the results to show instead of the grid.
    pub run: Option<String>,
    // Whether the page is on screen. Left, it stays up, hidden, with every pipeline kept live.
    pub shown: bool,
}

pub struct PipelinePage {
    // Every pipeline loaded so far, by name.
    pipelines: HashMap<String, PipelineSnapshot>,
    // How each pipeline's grid lays out at the width it has now, once measured. Until the one on
    // screen is, its grid is only its header: the rows go in after, laid out once, rather than
    // laid out and then restyled for the measured layout.
    layouts: HashMap<String, view::GridLayout>,
    // The pipeline on screen. Behind the URL while the one a tab chose loads or is measured.
    shown: Option<String>,
    notice: Option<String>,
    // The views of the run whose results show, once the boss has drawn them.
    views: Vec<view::View>,
    // A live event moves no column, so only a render after anything else measures again.
    measure: bool,
    main: NodeRef,
    // The off-screen grid a pipeline is measured in before it shows.
    staging: NodeRef,
    // Made once: a new callback would differ from the old one, and render every row again.
    cancel: Callback<String>,
    // The run whose results show instead of the grid.
    run: Option<String>,
    _resize: EventListener,
    _fonts: EventListener,
    // The stream of the pipeline the page opened on, until `all`'s snapshot has it too.
    first: Option<crate::Followed>,
    // Every pipeline's, opened once the first snapshot is in.
    all: Option<crate::Followed>,
}

pub enum PipelineMsg {
    First(Live<PipelineSnapshot>),
    All(Live<Vec<PipelineSnapshot>>),
    Notice(String),
    // The run the views were drawn for, so a late reply cannot land on another.
    Views(String, Vec<view::View>),
    StartRun,
    Cancel(String),
    // How the named pipeline's grid lays out, measured on screen or off.
    Measured(String, view::GridLayout),
    Resized,
}

impl Component for PipelinePage {
    type Message = PipelineMsg;
    type Properties = PipelineProps;

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link();
        let resized = link.callback(|()| PipelineMsg::Resized);
        // Names are measured in the embedded font, which may arrive after the first render.
        let fonts_loaded = link.callback(|()| PipelineMsg::Resized);
        let fonts = gloo::utils::document().fonts();
        let run = ctx.props().run.clone();
        draw_views(ctx, run.as_deref());
        // The streams stop only when the page is destroyed: left for another page, it stays live.
        let url = format!("/api/pipeline/{}/events", ctx.props().id);
        let first = follow(
            &url,
            link.callback(PipelineMsg::First),
            link.callback(PipelineMsg::Notice),
        );
        Self {
            pipelines: HashMap::new(),
            layouts: HashMap::new(),
            shown: None,
            notice: None,
            views: vec![],
            measure: true,
            main: NodeRef::default(),
            staging: NodeRef::default(),
            cancel: link.callback(PipelineMsg::Cancel),
            run,
            _resize: EventListener::new(&window(), "resize", move |_| resized.emit(())),
            _fonts: EventListener::new(&fonts, "loadingdone", move |_| fonts_loaded.emit(())),
            first: Some(first),
            all: None,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>, old: &Self::Properties) -> bool {
        // Hidden, nothing could be measured: the window may have changed size meanwhile.
        if ctx.props().shown && !old.shown {
            self.measure = true;
        }
        if ctx.props().id != old.id {
            self.settle(ctx);
            return true;
        }
        if ctx.props().run != old.run {
            self.run = ctx.props().run.clone();
            self.views.clear();
            draw_views(ctx, self.run.as_deref());
            return true;
        }
        ctx.props().shown != old.shown
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            // `all` has this pipeline's events now: one from here as well would apply twice.
            PipelineMsg::First(_) if self.first.is_none() => return false,
            PipelineMsg::First(Live::Snapshot(snapshot)) => {
                self.keep(snapshot);
                // Once there is a pipeline to show, every other loads behind it.
                if self.all.is_none() {
                    let link = ctx.link();
                    let on_live = link.callback(PipelineMsg::All);
                    let on_notice = link.callback(PipelineMsg::Notice);
                    self.all = Some(follow("/api/pipelines/events", on_live, on_notice));
                }
                self.settle(ctx);
            }
            PipelineMsg::All(Live::Snapshot(snapshots)) => {
                self.first = None;
                for snapshot in snapshots {
                    self.keep(snapshot);
                }
                self.settle(ctx);
            }
            PipelineMsg::First(Live::Event(event)) | PipelineMsg::All(Live::Event(event)) => {
                let Some((name, _)) = event.run() else {
                    return false;
                };
                let shown = self.shown.as_deref() == Some(name);
                if let Some(snapshot) = self.pipelines.get_mut(name) {
                    snapshot.apply(&event);
                }
                // Views drawn while the run ran showed it unfinished.
                if shown
                    && let (bettertest_shared::Event::RunFinished { run, .. }, Some(id)) =
                        (&event, &self.run)
                    && run.to_string() == *id
                {
                    draw_views(ctx, Some(id));
                }
                return shown;
            }
            PipelineMsg::Notice(text) => {
                self.notice = Some(text);
            }
            PipelineMsg::Views(run, views) => {
                if self.run.as_ref() != Some(&run) {
                    return false;
                }
                self.views = views;
            }
            PipelineMsg::Measured(name, layout) => {
                self.layouts.insert(name, layout);
                let before = self.shown.clone();
                self.settle(ctx);
                if self.shown != before {
                    // The off-screen measurement is the one this grid is drawn with. Measuring
                    // it on screen again would move it a frame later.
                    self.measure = false;
                    return true;
                }
            }
            PipelineMsg::Resized => {
                // The others were measured at the old size: each is measured again before it shows.
                self.layouts
                    .retain(|name, _| Some(name) == self.shown.as_ref());
            }
            PipelineMsg::Cancel(run) => {
                cancel_run(run, ctx.link().callback(PipelineMsg::Notice));
                return false;
            }
            PipelineMsg::StartRun => {
                let request = Request::post(&format!("/api/start-run/{}", ctx.props().id)).build();
                let notice = ctx.link().callback(PipelineMsg::Notice);
                act(request, "could not start a run", notice);
                return false;
            }
        }
        self.measure = true;
        true
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        // Hidden, everything measures 0 wide. It measures once shown again.
        if !ctx.props().shown {
            return;
        }
        // The next pipeline is in the page, hidden, at the width the grid will have. Measuring it
        // here and swapping on the next render is one visible frame: the grid that is up stays
        // until then.
        if let Some(pipeline) = self.staged(ctx) {
            let layout = self.measure_staged(pipeline).unwrap_or_default();
            ctx.link()
                .send_message(PipelineMsg::Measured(ctx.props().id.clone(), layout));
            return;
        }
        if !std::mem::take(&mut self.measure) || self.run.is_some() {
            return;
        }
        let Some(name) = &self.shown else { return };
        let (Some(main), Some(Ok(pipeline))) = (
            self.main.cast::<Element>(),
            self.pipelines.get(name).map(|shown| &shown.pipeline),
        ) else {
            return;
        };
        let layout = measure_grid(&main, pipeline);
        // Only a real change renders again, so this settles after one round.
        let moved = |a: view::Tilt, b: view::Tilt| {
            (a.angle - b.angle).abs() > 0.5 || (a.height - b.height).abs() > 1.0
        };
        let moved = self.layouts.get(name).is_none_or(|old| {
            moved(layout.stages, old.stages)
                || moved(layout.tasks, old.tasks)
                || layout.dense != old.dense
        });
        if moved {
            ctx.link()
                .send_message(PipelineMsg::Measured(name.clone(), layout));
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let start = html! {
            <button class="primary" onclick={ctx.link().callback(|_| PipelineMsg::StartRun)}>{ "New Run" }</button>
        };
        let shown = self
            .shown
            .as_ref()
            .and_then(|name| self.pipelines.get(name));
        let tabs = match shown {
            Some(snapshot) => view::tabs(&snapshot.pipelines, &snapshot.name),
            None => html! {},
        };
        let mut crumbs = html! {};
        let body = match (shown, &self.run) {
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
                    let layout = self.layouts.get(&snapshot.name);
                    let measured = layout.map(|_| &snapshot.runs);
                    let rows = measured.into_iter().flatten().map(|run| {
                        html! {
                            <RunRow key={run.id.to_string()} pipeline={pipeline.clone()} run={run.clone()} />
                        }
                    });
                    let layout = layout.cloned().unwrap_or_default();
                    view::grid(pipeline, &layout, html! { { for rows } })
                }
                Err(e) => view::notice(&Some(format!("the pipedef does not parse:\n{e}"))),
            },
        };
        // Off-screen, so the columns can be measured at the width the visible grid has. Nothing
        // on screen moves while it is here.
        let staging = match self.staged(ctx) {
            Some(pipeline) => html! {
                <div class="staging" aria-hidden="true" ref={self.staging.clone()}
                    style={STAGING}>
                    { view::grid(pipeline, &view::GridLayout::default(), html! {}) }
                </div>
            },
            None => html! {},
        };
        html! {
            <>
                { view::header(crumbs, start, account()) }
                { view::notice(&self.notice) }
                <main ref={self.main.clone()}>{ body }</main>
                { tabs }
                { staging }
            </>
        }
    }
}

/// Where the next grid is measured: off-screen, out of sight and out of reach.
const STAGING: &str =
    "position:fixed;left:0;top:0;visibility:hidden;pointer-events:none;overflow:hidden";

impl PipelinePage {
    /// Holds `snapshot` in place of what the page had of its pipeline.
    fn keep(&mut self, snapshot: PipelineSnapshot) {
        let name = snapshot.name.clone();
        let old = self.pipelines.get(&name);
        // Another pipedef lays out another way. The one on screen is measured again as it renders.
        if old.is_some_and(|old| old.pipeline != snapshot.pipeline)
            && self.shown.as_ref() != Some(&name)
        {
            self.layouts.remove(&name);
        }
        // Unchanged, the rows on screen stay as they are rather than render again.
        if self.pipelines.get(&name) != Some(&snapshot) {
            self.pipelines.insert(name, snapshot);
        }
    }

    /// Shows the pipeline the URL names once it can paint whole: measured, with nothing to
    /// measure (its pipedef does not parse), or the first the page shows, whose rows go in once
    /// it is measured. Until then the grid that is up stays, and the next is measured off-screen.
    fn settle(&mut self, ctx: &Context<Self>) {
        let id = &ctx.props().id;
        let Some(snapshot) = self.pipelines.get(id) else {
            return;
        };
        if self.shown.as_ref() == Some(id) {
            return;
        }
        if self.shown.is_none() || snapshot.pipeline.is_err() || self.layouts.contains_key(id) {
            self.shown = Some(id.clone());
            self.run = ctx.props().run.clone();
            self.notice = None;
            self.views.clear();
            draw_views(ctx, self.run.as_deref());
        }
    }

    /// The pipeline the URL names, loaded but not shown yet: measured off-screen first.
    fn staged(&self, ctx: &Context<Self>) -> Option<&Pipeline> {
        let id = &ctx.props().id;
        if self.shown.as_ref() == Some(id) {
            return None;
        }
        self.pipelines.get(id)?.pipeline.as_deref().ok()
    }

    /// The layout `pipeline` will have, measured in a box as wide as the grid on screen.
    fn measure_staged(&self, pipeline: &Pipeline) -> Option<view::GridLayout> {
        let stage = self.staging.cast::<Element>()?;
        let main = self.main.cast::<Element>()?;
        let width = main
            .query_selector(".grid")
            .ok()
            .flatten()
            .filter(|grid| grid.client_width() > 0)
            .map(|grid| grid.client_width())
            .unwrap_or_else(|| main.client_width().saturating_sub(32).max(1));
        let _ = stage.set_attribute("style", &format!("{STAGING};width:{width}px"));
        Some(measure_grid(&stage, pipeline))
    }
}

/// How the grid in `root` lays its names out. Rotation leaves offset sizes alone, so these are the
/// flat sizes whatever the angle.
fn measure_grid(root: &Element, pipeline: &Pipeline) -> view::GridLayout {
    let sizes = |selector: &str, size: fn(&HtmlElement) -> i32| -> Vec<f64> {
        let Ok(list) = root.query_selector_all(selector) else {
            return vec![];
        };
        (0..list.length())
            .filter_map(|i| list.item(i)?.dyn_into::<HtmlElement>().ok())
            .map(|e| size(&e) as f64)
            .collect()
    };
    let columns = sizes(".grid th.task", |e| e.client_width());
    let labels = sizes(".grid th.task span", |e| e.offset_width());
    let line = sizes(".grid th.task span", |e| e.offset_height());
    let line = line.first().copied().unwrap_or(0.0);
    // A stage whose columns are narrower than a line cannot show its tasks' names, even
    // upright: its tasks run together and leave their names to the tooltips.
    let mut dense = vec![];
    let mut shown = vec![];
    let mut at = 0;
    for stage in &pipeline.stages {
        let packed = columns.get(at).is_some_and(|width| *width < line);
        dense.push(packed);
        let end = (at + stage.tasks.len()).min(labels.len());
        let names = labels.get(at..end).unwrap_or_default();
        shown.extend(names.iter().map(|label| (!packed).then_some(*label)));
        at += stage.tasks.len();
    }
    let stage_columns = sizes(".grid th.stage", |e| e.client_width());
    let stage_labels = sizes(".grid th.stage span", |e| e.offset_width());
    let stage_labels: Vec<_> = stage_labels.into_iter().map(Some).collect();
    view::GridLayout {
        // Stage names are as tall as task names.
        stages: view::label_layout(&stage_columns, &stage_labels, line),
        tasks: view::label_layout(&columns, &shown, line),
        dense,
    }
}

/// Has the boss draw the views of `run` for its results.
fn draw_views(ctx: &Context<PipelinePage>, run: Option<&str>) {
    let Some(run) = run else { return };
    let request = Request::get(&format!("/api/run/{run}/views")).build();
    let run = run.to_string();
    let drawn = move |views| PipelineMsg::Views(run, views);
    load(ctx.link(), request, drawn, |e| {
        PipelineMsg::Notice(format!("no views: {e}"))
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
/// thousands of tasks, rendering every row for every event kept the browser busy. Its cells are
/// HTML that run_row builds, set in one go. That happens while rendering, not in `rendered`:
/// Yew may give the browser a frame before calling `rendered`, which painted the next pipeline's
/// header over the previous one's rows.
struct RunRow;

#[derive(Properties)]
struct RunRowProps {
    pipeline: Arc<Pipeline>,
    run: Arc<Run>,
}

impl PartialEq for RunRowProps {
    fn eq(&self, other: &Self) -> bool {
        // The snapshot replaces a run that changes, so the pointer says whether it did.
        Arc::ptr_eq(&self.pipeline, &other.pipeline) && Arc::ptr_eq(&self.run, &other.run)
    }
}

impl Component for RunRow {
    type Message = ();
    type Properties = RunRowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self
    }

    /// Only a changed run renders: the row's cells are this run's now.
    fn view(&self, ctx: &Context<Self>) -> Html {
        let Ok(row) = gloo::utils::document().create_element("tr") else {
            return html! {};
        };
        let props = ctx.props();
        row.set_inner_html(&view::run_row(&props.pipeline, &props.run));
        Html::VRef(row.into())
    }
}
