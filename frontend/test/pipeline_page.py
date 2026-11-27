"""The pipeline page in a browser, with a pipeline of 41 stages (the last of them one task wide)
and a dozen tabs:

- at 320, 375, 390, 768, 1024 and 1440px, neither the page nor anything on it scrolls sideways or reaches
  past the screen, the grid and a run's results alike, and every stage name shows whole;
- a skipped task's cell is no link and does not answer the pointer;
- every other run cell lights up when hovered, turns white when pressed and fades back once
  released, like a button (brand guide, "Hover, press, release");
- run ids are muted text, not link-colored;
- switching to a big-ish pipeline (5 stages of 10 tasks, 50 runs: 2 500 cells) draws all its runs
  within SWITCH_MS of the tab's click.
- switching tabs, and going back and forward, never paints an intermediate frame: the grid does
  not empty, no skeleton appears, and the other pipeline is not shown half-drawn. The snapshot
  is held back so the wait is real. Chromium (on a CPU slowed 20 times) and WebKit, at 390 and
  1280.

The boss is shell.py's stand-in: frontend/dist, plus this pipeline's snapshot. Like shell.py, it
runs in Playwright's image (see shell_nav.rs, which runs both in cargo test).
"""

import os
import re
import sys
import threading

import shell

STAGES = 41
# The run that fails, fails here, so the stages after it are skipped.
FAILS_AT = 30
WIDTHS = [320, 375, 390, 768, 1024, 1440]
MUTED = "rgb(138, 138, 138)"
WHITE = "rgb(255, 255, 255)"


def tasks_in(stage):
    # Stage 7 has too many tasks to name; the rest have one to four.
    return 150 if stage == 7 else 1 + stage % 4


def task_run(run, stage, task, state):
    over = state in ("pass", "fail")
    return {
        "id": f"{run:08x}-0000-4000-8{stage:03x}-{task:012x}",
        "name": f"check_{task}",
        "state": state,
        "worker": None if state in ("pending", "skipped") else "http://worker:7358",
        "started": None if state in ("pending", "skipped") else 1_000,
        "ended": 2_500 if over else None,
        "carried_from": None,
        "tests": None,
    }


def run(number, state, state_of):
    stages = [
        {
            "name": stage_name(s),
            "tasks": [
                task_run(number, s, t, state_of(s, t)) for t in range(tasks_in(s))
            ],
        }
        for s in range(STAGES)
    ]
    return {
        "id": run_id(number),
        "pipeline": "many_stages",
        "state": state,
        "commit": None,
        "stages": stages,
    }


def run_id(number):
    return f"6dfde{number:x}00-0000-4000-8000-000000000000"


def stage_name(stage):
    return f"VerifyEverythingInStep{stage:02}"


def failing(stage, task):
    if stage < FAILS_AT:
        return "pass"
    if stage == FAILS_AT:
        return "fail" if task == 0 else "pass"
    return "skipped"


def running(stage, task):
    return "pass" if stage < 5 else "running" if stage == 5 else "pending"


def body():
    task = {
        "worker": "http://worker:7358",
        "labels": [],
        "image": None,
        "vm": None,
        "arch": None,
        "timeout": None,
        "sandbox": None,
    }
    stages = [
        {
            "name": stage_name(s),
            "sandbox": None,
            "tasks": [{"name": f"check_{t}", **task} for t in range(tasks_in(s))],
        }
        for s in range(STAGES)
    ]
    return {
        "name": "many_stages",
        "pipelines": ["many_stages", "big_ish"] + [f"another_pipeline_{n}" for n in range(10)],
        "pipeline": {"Ok": {"stages": stages, "sandboxes": {}}},
        "runs": [
            run(3, "running", running),
            run(2, "fail", failing),
            run(1, "pass", lambda s, t: "pass"),
        ],
    }


SIDEWAYS = """() => {
    const problems = [];
    const page = document.scrollingElement;
    const width = page.clientWidth;
    const name = e => e.tagName.toLowerCase() + (e.className ? '.' + e.className : '');
    if (page.scrollWidth > width) {
        problems.push(`the page is ${page.scrollWidth}px wide`);
    }
    for (const e of document.querySelectorAll('body *')) {
        const style = getComputedStyle(e);
        const scrolls = style.overflowX === 'auto' || style.overflowX === 'scroll';
        if (scrolls && e.scrollWidth > e.clientWidth) {
            problems.push(`${name(e)} scrolls sideways: ${e.scrollWidth}px in ${e.clientWidth}px`);
        }
        const box = e.getBoundingClientRect();
        const shown = style.visibility !== 'hidden' && box.width > 0;
        if (shown && (box.left < -0.5 || box.right > width + 0.5)) {
            problems.push(`${name(e)} reaches ${box.left.toFixed(1)}..${box.right.toFixed(1)}px`);
        }
    }
    for (const stage of document.querySelectorAll('.grid th.stage span')) {
        if (stage.scrollWidth > stage.clientWidth) {
            problems.push(`stage ${stage.textContent} is cut off`);
        }
    }
    return problems;
}"""


# From the tab's click to every run's row painted: the page's work once the snapshot is in (the
# stand-in boss answers at once). The quickest of a few, so a busy machine cannot fail it.
SWITCH_MS = 200
BIG_STAGES, BIG_TASKS, BIG_RUNS = 5, 10, 50


def big_ish():
    task = {"worker": None, "labels": ["x86"], "image": None, "vm": None, "arch": None,
            "timeout": None, "sandbox": None}
    stages = [
        {"name": f"Step{s}", "sandbox": None,
         "tasks": [{"name": f"job_{s}_{t}", **task} for t in range(BIG_TASKS)]}
        for s in range(BIG_STAGES)
    ]
    runs = [
        {
            "id": f"{r:08x}-1111-4000-8000-000000000000",
            "pipeline": "big_ish",
            "state": "pass",
            "commit": None,
            "stages": [
                {"name": f"Step{s}",
                 "tasks": [{"id": f"{r:08x}-0000-4000-8{s:03x}-{t:012x}", "name": f"job_{s}_{t}",
                            "state": "fail" if (r + t) % 7 == 0 else "pass",
                            "worker": "http://worker:7358", "started": 1_000 + r,
                            "ended": 2_500 + r * t} for t in range(BIG_TASKS)]}
                for s in range(BIG_STAGES)
            ],
        }
        for r in range(BIG_RUNS)
    ]
    return {
        "name": "big_ish",
        "pipelines": ["many_stages", "big_ish"] + [f"another_pipeline_{n}" for n in range(10)],
        "pipeline": {"Ok": {"stages": stages, "sandboxes": {}}},
        "runs": runs,
    }


SWITCH = """(name) => new Promise(done => {
    const start = performance.now();
    [...document.querySelectorAll('.tab')].find(a => a.textContent === name).click();
    const drawn = () => {
        const rows = document.querySelectorAll('.grid tbody tr');
        const cells = document.querySelectorAll('.grid tbody td');
        if (rows.length === %d && cells.length === %d) {
            requestAnimationFrame(() => done(performance.now() - start));
        } else if (performance.now() - start > 5000) {
            done(`not drawn after 5s: ${rows.length} rows, ${cells.length} cells`);
        } else {
            requestAnimationFrame(drawn);
        }
    };
    drawn();
})""" % (BIG_RUNS, BIG_RUNS * BIG_STAGES * BIG_TASKS)


def switching_pipelines_draws_at_once(browser, base):
    page = browser.new_page(viewport={"width": 1440, "height": 900})
    try:
        page.goto(f"{base}/pipeline/many_stages")
        page.wait_for_selector(".grid tbody td")
        took = []
        for _ in range(5):
            took.append(page.evaluate(SWITCH, "big_ish"))
            assert not isinstance(took[-1], str), took[-1]
            page.locator(".tab", has_text="many_stages").click()
            page.wait_for_selector(".grid th.stage >> text=VerifyEverythingInStep00")
        print(f"switching to big_ish took {[round(t) for t in took]} ms")
        assert min(took) < SWITCH_MS, f"switching to big_ish took {min(took):.0f} ms at best"
    finally:
        page.close()


# Sets window.__btLoaded once the stream of every pipeline has handed the page its snapshot. The
# app's own listener runs in the same dispatch, and its work in microtasks after it: this timeout
# comes later still.
LOADED = """(() => {
    const Original = window.EventSource;
    window.EventSource = class extends Original {
        constructor(url, options) {
            super(url, options);
            if (String(url).endsWith('/api/pipelines/events')) {
                this.addEventListener('message', () => setTimeout(() => { window.__btLoaded = true; }));
            }
        }
    };
})()"""

# Clicks pipeline `name`'s tab, then looks at every frame until its grid shows: which frame that was,
# and what it showed.
SWITCH_FRAMES = """(name) => new Promise(done => {
    [...document.querySelectorAll('nav.tabs a.tab')].find(a => a.textContent === name).click();
    let frames = 0;
    const look = () => {
        frames += 1;
        const current = document.querySelector('nav.tabs a.tab.current');
        const rows = [...document.querySelectorAll('main .grid tbody tr')];
        if ((current && current.textContent === name && rows.length) || frames === 120) {
            done({
                frames,
                current: current && current.textContent,
                stage: document.querySelector('main .grid th.stage span')?.textContent,
                rows: rows.length,
                first: rows[0]?.querySelector('th.run')?.textContent,
            });
        } else {
            requestAnimationFrame(look);
        }
    };
    requestAnimationFrame(look);
})"""


def loaded_tabs_switch_from_memory(browser, base):
    """Once the page has loaded every pipeline behind the first, a tab paints its pipeline in the
    next frame and asks the network for nothing, whether it has shown that pipeline before (its
    layout kept) or not (measured off-screen first)."""
    context = browser.new_context(viewport={"width": 1440, "height": 900})
    context.add_init_script(LOADED)
    page = context.new_page()
    fetched = []
    page.on("request", lambda request: fetched.append(request.url))
    try:
        page.goto(f"{base}/pipeline/many_stages")
        page.wait_for_selector(".grid tbody td")
        page.wait_for_function("() => window.__btLoaded === true")
        page.evaluate("document.fonts.ready")
        big_first = big_ish()["runs"][0]["id"][:6]
        many_first = body()["runs"][0]["id"][:6]
        for name, stage, rows, first in [
            ("big_ish", "Step0", BIG_RUNS, big_first),
            ("many_stages", stage_name(0), 3, many_first),
            ("big_ish", "Step0", BIG_RUNS, big_first),
        ]:
            fetched.clear()
            shown = page.evaluate(SWITCH_FRAMES, name)
            page.wait_for_timeout(300)
            print(f"  -> {name}: {shown}, fetched {fetched}")
            assert not fetched, f"switching to {name} fetched {fetched}"
            want = {"frames": 1, "current": name, "stage": stage, "rows": rows, "first": first}
            assert shown == want, f"switching to {name} showed {shown}, not {want}"
            assert page.url.endswith(f"/pipeline/{name}"), page.url
    finally:
        context.close()


def settle(page, url, ready):
    page.goto(url)
    page.wait_for_selector(ready)
    page.evaluate("document.fonts.ready")
    # The grid measures its names after rendering, then renders again.
    page.wait_for_timeout(300)


def nothing_scrolls_sideways(browser, base):
    failures = []
    for width in WIDTHS:
        phone = width < 768
        context = browser.new_context(
            viewport={"width": width, "height": 800}, is_mobile=phone, has_touch=phone
        )
        page = context.new_page()
        for path, ready in [
            ("/pipeline/many_stages", ".grid tbody td"),
            (f"/pipeline/many_stages?run={run_id(2)}", ".results"),
        ]:
            settle(page, base + path, ready)
            failures += [f"{width}px {path}: {p}" for p in page.evaluate(SIDEWAYS)]
            if ready == ".grid tbody td":
                stages = page.locator(".grid th.stage").count()
                if stages != STAGES:
                    failures.append(
                        f"{width}px: {stages} stages in the grid, not {STAGES}"
                    )
        context.close()
    assert not failures, "\n".join(failures)


def background(locator):
    return locator.evaluate("e => getComputedStyle(e).backgroundColor")


def cells_answer_like_buttons(browser, base):
    page = browser.new_page(
        viewport={"width": 1440, "height": 800}, color_scheme="dark"
    )
    settle(page, base + "/pipeline/many_stages", ".grid tbody td")
    run_ids = page.locator(".grid th.run a")
    assert run_ids.count() == 3
    color = run_ids.first.evaluate("e => getComputedStyle(e).color")
    assert color == MUTED, f"run id is {color}, not muted {MUTED}"

    skipped = page.locator(".cell.skipped")
    assert skipped.count() == sum(tasks_in(s) for s in range(FAILS_AT + 1, STAGES))
    assert page.locator(".cell.skipped a").count() == 0, "a skipped cell is a link"
    cell = skipped.first
    cursor = cell.evaluate("e => getComputedStyle(e).cursor")
    assert cursor != "pointer", "a skipped cell shows the pointer cursor"
    still = background(cell)
    cell.hover()
    assert background(cell) == still, "a hovered skipped cell changes"
    page.mouse.down()
    assert background(cell) == still, "a pressed skipped cell changes"
    page.mouse.up()

    for state in ["pass", "fail", "running"]:
        link = page.locator(f".cell.{state} a").first
        page.mouse.move(0, 0)
        rest = background(link)
        link.hover()
        lit = background(link)
        assert lit not in (rest, WHITE), f"a hovered {state} cell stays {rest}"
        page.mouse.down()
        pressed = background(link)
        assert pressed == WHITE, f"a pressed {state} cell is {pressed}, not white"
        # Dragged off before letting go, so the press opens nothing but still fades.
        page.mouse.move(0, 0)
        page.mouse.up()
        fade = link.evaluate(
            "e => e.classList.contains('released') && getComputedStyle(e).animationName"
        )
        assert fade == "released", (
            f"a released {state} cell does not fade back ({fade})"
        )
    page.close()


# Two pipelines a switch can be told apart by, even half-drawn: different stage names, task names
# and row counts. Short names, so measuring them does not tilt a header and move the grid.
# Pipedef tasks name labels only.
RED_STAGES = [("Red", ["build", "test"]), ("Rose", ["lint", "pack"])]
BLUE_STAGES = [("Blue", ["boot"]), ("Bay", ["probe"]), ("Bolt", ["ship"])]
TABS = ["red", "blue"]
# The stand-in holds back the other pipelines this long after the tab's click, inside the
# 500–1000ms the wait has to be real for.
HOLD = 0.8
# How many times slower than this machine Chromium runs the switches.
CPU_SLOWDOWN = 20


def flicker_pipeline(name, stages, runs):
    task_def = {
        "worker": None,
        "labels": ["x86"],
        "image": None,
        "vm": None,
        "arch": None,
        "timeout": None,
        "sandbox": None,
    }
    pipe_stages = [
        {
            "name": stage,
            "sandbox": None,
            "tasks": [{"name": task, **task_def} for task in tasks],
        }
        for stage, tasks in stages
    ]

    def one_run(number):
        stages_out = []
        n = 0
        for stage, tasks in stages:
            row = []
            for task in tasks:
                row.append(
                    {
                        "id": f"{number:08x}-0000-4000-8{n:03x}-{number:012x}",
                        "name": task,
                        "state": "pass",
                        "worker": None,
                        "started": 1_000,
                        "ended": 2_000,
                        "carried_from": None,
                        "tests": None,
                    }
                )
                n += 1
            stages_out.append({"name": stage, "tasks": row})
        # Run ids are uuids, so no two pipelines share one.
        return {
            "id": f"{number:08x}-{TABS.index(name) + 1:04x}-4000-8000-000000000000",
            "pipeline": name,
            "state": "pass",
            "commit": None,
            "stages": stages_out,
        }

    return {
        "name": name,
        "pipelines": TABS,
        "pipeline": {"Ok": {"stages": pipe_stages, "sandboxes": {}}},
        "runs": [one_run(n) for n in range(runs, 0, -1)],
    }


def expect(name, stages, runs):
    tasks = [task for _stage, tasks in stages for task in tasks]
    return {
        "current": name,
        "tabs": TABS,
        "stages": [stage for stage, _tasks in stages],
        "tasks": tasks,
        "rows": runs,
        "cells": runs * len(tasks),
    }


RED = expect("red", RED_STAGES, 3)
BLUE = expect("blue", BLUE_STAGES, 1)

# Samples every mutation and every frame. `target` is the pipeline that must end up showing.
# A hidden staging grid, if one is ever mounted, is not the page.
WATCH = r"""(target) => {
    if (window.__btStop) window.__btStop();
    const state = {
        samples: [], stop: false, done: false, target,
        stable: 0, t0: performance.now(), action: null,
    };
    window.__bt = state;
    const visible = (el) => {
        for (let n = el; n && n !== document.documentElement; n = n.parentElement) {
            if (n.classList && n.classList.contains("staging")) return false;
            const style = getComputedStyle(n);
            if (style.display === "none" || style.visibility === "hidden") return false;
        }
        return true;
    };
    const rect = (el) => {
        if (!el) return null;
        const r = el.getBoundingClientRect();
        const round = (n) => Math.round(n * 100) / 100;
        return [round(r.x), round(r.y), round(r.width), round(r.height)];
    };
    const read = (why) => {
        const grids = [...document.querySelectorAll(".grid")].filter(visible);
        const grid = grids[0] || null;
        const tabs = [...document.querySelectorAll("nav.tabs a.tab")].filter(visible);
        const current = tabs.filter((t) => t.classList.contains("current")).map((t) => t.textContent);
        return {
            why,
            t: Math.round(performance.now() - state.t0),
            skel: !!document.querySelector(".frame-skel"),
            visibleGrids: grids.length,
            stages: grid ? [...grid.querySelectorAll("th.stage span")].map((e) => e.textContent) : [],
            tasks: grid ? [...grid.querySelectorAll("th.task span")].map((e) => e.textContent) : [],
            rows: grid ? grid.querySelectorAll("tbody tr").length : 0,
            cells: grid ? grid.querySelectorAll("tbody td").length : 0,
            tabs: tabs.map((t) => t.textContent),
            current: current.length === 1 ? current[0] : current.join(","),
            header: rect(document.querySelector("header")),
            grid: rect(grid),
        };
    };
    window.__btRead = () => read("now");
    const sample = (why) => {
        if (state.stop || state.samples.length > 4000) return;
        const row = read(why);
        state.samples.push(row);
        const want = state.target;
        const same = row.current === want.current && row.rows === want.rows && row.cells === want.cells
            && JSON.stringify(row.tabs) === JSON.stringify(want.tabs)
            && JSON.stringify(row.stages) === JSON.stringify(want.stages)
            && JSON.stringify(row.tasks) === JSON.stringify(want.tasks);
        state.stable = same ? state.stable + 1 : 0;
        if (why === "raf" && state.stable >= 3) state.done = true;
    };
    const obs = new MutationObserver(() => sample("mut"));
    obs.observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
    const tick = () => {
        if (state.stop) return;
        sample("raf");
        requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    window.__btStop = () => { state.stop = true; obs.disconnect(); };
}"""


def matches(sample, ident):
    box = sample["grid"]
    return (
        not sample["skel"]
        and sample["visibleGrids"] == 1
        and sample["current"] == ident["current"]
        and sample["tabs"] == ident["tabs"]
        and sample["stages"] == ident["stages"]
        and sample["tasks"] == ident["tasks"]
        and sample["rows"] == ident["rows"]
        and sample["cells"] == ident["cells"]
        and sample["header"] is not None
        and box is not None
        and box[2] > 20
        and box[3] > 20
    )


def jumped(a, b):
    if not a or not b:
        return True
    return any(abs(x - y) > 1 for x, y in zip(a, b))


def summarize(sample):
    return (
        f"t={sample['t']} {sample['why']} current={sample['current']!r} "
        f"stages={sample['stages']} tasks={sample['tasks']} "
        f"rows={sample['rows']} cells={sample['cells']} tabs={sample['tabs']} "
        f"grids={sample['visibleGrids']} skel={sample['skel']} "
        f"header={sample['header']} grid={sample['grid']}"
    )


def assert_no_flicker(samples, before, after, action_at, waits):
    bad = []
    seen_before = False
    seen_after = False
    identity = None
    header = None
    grid_box = None
    held = None
    for sample in samples:
        if sample["t"] < action_at:
            continue
        which = "before" if matches(sample, before) else "after" if matches(sample, after) else None
        problems = []
        if sample["skel"]:
            problems.append("skeleton is showing")
        if which is None:
            problems.append("not a complete pipeline")
        elif which == "before" and seen_after:
            problems.append("the old pipeline came back after the new one")
        if which == "before":
            seen_before = True
        if which == "after" and held is None:
            held = sample["t"] - action_at
            seen_after = True
        if which == "after":
            seen_after = True
        # The chrome does not move. The grid may change size only in the frame the pipeline does.
        if header is not None and jumped(header, sample["header"]):
            problems.append(f"header jumped {header} -> {sample['header']}")
        if which is not None and which == identity and grid_box is not None and jumped(grid_box, sample["grid"]):
            problems.append(f"grid jumped {grid_box} -> {sample['grid']}")
        if which is not None:
            identity = which
            grid_box = sample["grid"]
        if sample["header"] is not None:
            header = sample["header"]
        if problems:
            bad.append(f"{summarize(sample)} :: {', '.join(problems)}")
    assert not bad, f"{len(bad)} intermediate frame(s):\n" + "\n".join(bad[:12])
    assert seen_after, "the pipeline switched to never arrived"
    if waits:
        assert seen_before, "the switch never showed the pipeline it left"
        assert held is not None and held >= 500, (
            f"the new pipeline showed {held}ms after the switch; "
            f"its snapshot is held {int(HOLD * 1000)}ms, so this did not wait"
        )
    return held


def switch(page, how, name, before, after, shots, width):
    """`click` switches to a pipeline the page has not loaded yet, which the stand-in holds back
    for HOLD seconds; back and forward to ones it has shown already."""
    page.evaluate(WATCH, after)
    page.wait_for_timeout(50)
    action_at = page.evaluate(
        "() => { window.__bt.action = performance.now() - window.__bt.t0; return window.__bt.action; }"
    )
    waits = how == "click"
    if how == "click":
        page.locator("nav.tabs a.tab", has_text=re.compile(rf"^{re.escape(name)}$")).click()
        threading.Timer(HOLD, shell.RELEASED.set).start()
    elif how == "back":
        page.go_back(wait_until="commit")
    else:
        page.go_forward(wait_until="commit")
    # One frame while the snapshot is still held. Screenshot time itself is not part of the wait
    # the samples measure; the samples cover every mutation and frame until the new grid is up.
    if shots and how == "click":
        page.wait_for_timeout(250)
        page.screenshot(path=os.path.join(shots, f"mid-{width}.png"))
    page.wait_for_function("() => window.__bt.done", timeout=8000)
    samples = page.evaluate("() => window.__bt.samples")
    page.evaluate("() => window.__btStop()")
    assert page.url.endswith(f"/pipeline/{name}"), page.url
    held = assert_no_flicker(samples, before, after, action_at, waits)
    print(f"  {how} -> {name} held the previous pipeline {held}ms across {len(samples)} samples")


def switching_tabs_never_flickers(chromium, webkit, base):
    """Clicking a pipeline tab, and the browser's back and forward, paints no intermediate frame."""
    shell.PIPELINES["red"] = shell.snapshot(flicker_pipeline("red", RED_STAGES, 3))
    shell.PIPELINES["blue"] = shell.snapshot(flicker_pipeline("blue", BLUE_STAGES, 1))
    shots = os.environ.get("BT_SHOTS")
    try:
        for browser_name, browser in (("chromium", chromium), ("webkit", webkit)):
            for width, mobile in ((390, True), (1280, False)):
                context = browser.new_context(
                    viewport={"width": width, "height": 800},
                    is_mobile=mobile,
                    has_touch=mobile,
                )
                page = context.new_page()
                if browser_name == "chromium":
                    # Yew lets the browser paint once its work has taken 16ms. On a slow CPU every
                    # switch takes that long, so a frame painted half-done shows every time, not
                    # only when the machine happens to be busy.
                    cdp = context.new_cdp_session(page)
                    cdp.send("Emulation.setCPUThrottlingRate", {"rate": CPU_SLOWDOWN})
                try:
                    shell.RELEASED.clear()
                    page.goto(f"{base}/pipeline/red", wait_until="domcontentloaded")
                    page.wait_for_selector(".grid tbody td")
                    page.evaluate("() => document.fonts.ready")
                    page.wait_for_timeout(300)
                    print(f"{browser_name} {width}px")
                    switch(page, "click", "blue", RED, BLUE, shots, width)
                    switch(page, "back", "red", BLUE, RED, None, width)
                    switch(page, "forward", "blue", RED, BLUE, None, width)
                finally:
                    context.close()
    finally:
        shell.RELEASED.set()


def main():
    # The browser side runs in Playwright's image, which has no trunk.
    if shell.dist_stale():
        raise SystemExit(
            "frontend/dist is missing or older than frontend/: run ./build.sh (or frontend/build.sh) first"
        )
    shell.PIPELINES["many_stages"] = shell.snapshot(body())
    shell.PIPELINES["big_ish"] = shell.snapshot(big_ish())
    server, base = shell.start_server()
    from playwright.sync_api import sync_playwright

    failed = 0
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                args=["--no-sandbox", "--disable-dev-shm-usage"]
            )
            webkit = playwright.webkit.launch()

            def tabs_never_flicker(browser, base):
                switching_tabs_never_flickers(browser, webkit, base)

            tests = [
                nothing_scrolls_sideways,
                cells_answer_like_buttons,
                switching_pipelines_draws_at_once,
                loaded_tabs_switch_from_memory,
                tabs_never_flicker,
            ]
            for test in tests:
                try:
                    test(browser, base)
                    print(f"PASS {test.__name__}")
                except AssertionError as error:
                    failed += 1
                    print(f"FAIL {test.__name__}\n{error}")
            webkit.close()
            browser.close()
    finally:
        server.shutdown()
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
