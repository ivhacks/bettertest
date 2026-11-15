#!/usr/bin/env python3
"""How long the five pages take in Chromium, against the real binary.

    python3 speed.py BINARY cold         each page fully cold: a new browser, first paint and
                                         fully loaded
    python3 speed.py BINARY tabs         from one pipeline's tab to another's, fully loaded
    python3 speed.py BINARY navigation   from the pipeline page to each other page, and back with
                                         the back button, fully loaded

Run it in Playwright's image (bettertest-test-playwright, or .bettertest/playwright.Dockerfile).
The first run starts `BINARY unified` on two small pipelines, gives each RUNS runs, and leaves it
running, so later runs in the same container measure the same boss and the same data.

First paint is Chromium's first-contentful-paint: index.html paints nothing, so the first paint
of anything is the app's header. Fully loaded is the moment the page's data is on screen (LOADED),
checked on every animation frame from the moment the navigation starts. Each time is the fastest
of TRIES, since a slower try measures the machine, not the page. Each line says the time and its
limit, `<what>: <ms> ms, limit <ms> ms`, which the CI's views draw; any over its limit fails.
"""

import json
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

BOSS = "http://localhost:7357"
RUNS = 10
TRIES = 5
COLD_TRIES = 3

# About three times what a quiet x86 machine takes (cold ~90 ms, in the app ~12 ms), so a busy CI
# worker passes and a real slowdown does not.
LIMITS = {
    "cold first paint": 300,
    "cold loaded": 400,
    "tab switch": 100,
    "navigate": 100,
    "back": 100,
}

PIPEDEF = """
from bettertest import Labels, Stage, task


class Label(Labels):
    x86 = "x86"


class Build(Stage):
    labels = (Label.x86,)

    @task()
    def compile():
        for i in range(20):
            print(f"compiling {i}")
        print("done")


class Test(Stage):
    labels = (Label.x86,)

    @task()
    def unit():
        print("done")

    @task()
    def lint():
        print("done")

    @task()
    def e2e():
        print("done")
"""


def rows(pipeline):
    return f"document.querySelectorAll(\"tbody th.run a[href^='/pipeline/{pipeline}?']\").length >= {RUNS}"


# What each page shows once its data is in.
LOADED = {
    "pipeline": rows("app"),
    "task": "document.querySelector('pre.log')?.textContent.includes('done')",
    "workers": "document.querySelector('table.workers tbody tr') !== null",
    "settings": "document.querySelector('form.settings input') !== null",
    "users": "document.querySelector('table.users tbody tr') !== null",
}


def get(path):
    return json.load(urllib.request.urlopen(BOSS + path))


def post(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        BOSS + path,
        data=data,
        method="POST",
        headers={"content-type": "application/json"},
    )
    return json.load(urllib.request.urlopen(request))


def boss_up():
    try:
        get("/api/pipelines")
        return True
    except OSError:
        return False


def start(binary):
    """Starts the boss and its worker on two pipelines with RUNS finished runs each, unless an
    earlier run in this container already did."""
    if boss_up():
        return
    where = Path(tempfile.mkdtemp())
    for name in ["app", "docs"]:
        (where / f"{name}.py").write_text(PIPEDEF)
    log = open(where / "boss.log", "w")
    subprocess.Popen(
        [binary, "unified", "app.py", "docs.py"],
        cwd=where,
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    # Up once the boss answers and its worker has connected.
    deadline = time.monotonic() + 30
    while not (boss_up() and any(w["info"] for w in get("/api/workers"))):
        if time.monotonic() > deadline:
            raise SystemExit(f"no boss with a worker after 30s; see {where}/boss.log")
        time.sleep(0.05)
    for name in ["app", "docs"]:
        for _ in range(RUNS):
            run = post(f"/api/start-run/{name}")
            for line in urllib.request.urlopen(f"{BOSS}/api/run/{run['id']}/events"):
                if b"RunFinished" in line:
                    break


def a_task():
    run = get("/api/run/" + newest_run("app"))
    return run["stages"][0]["tasks"][0]["id"]


def newest_run(pipeline):
    events = urllib.request.urlopen(f"{BOSS}/api/pipeline/{pipeline}/events")
    for line in events:
        if line.startswith(b"data: "):
            snapshot = json.loads(line[6:])["Snapshot"]
            return snapshot["runs"][0]["id"]
    raise SystemExit(f"no snapshot for {pipeline}")


def timed(page, action, cond):
    """Milliseconds from doing `action` (JS) until `cond` (JS) holds, checked every frame."""
    return page.evaluate(
        f"""(cond) => new Promise((done) => {{
            const met = new Function("return " + cond);
            const t0 = performance.now();
            {action};
            (function check() {{ if (met()) done(performance.now() - t0); else requestAnimationFrame(check); }})();
        }})""",
        cond,
    )


def cold(chromium, pages):
    """A new browser for every load: nothing cached, no connection open."""
    for name, path in pages:
        paints, loads = [], []
        for _ in range(COLD_TRIES):
            browser = chromium.launch()
            page = browser.new_page()
            # Watches from before the document's first script, so the time is the navigation's.
            page.add_init_script(
                f"""(function check() {{
                    if ({LOADED[name]}) window.loadedAt = performance.now();
                    else requestAnimationFrame(check);
                }})();"""
            )
            page.goto(BOSS + path, wait_until="commit")
            loads.append(page.wait_for_function("window.loadedAt").json_value())
            paints.append(
                page.wait_for_function(
                    "performance.getEntriesByName('first-contentful-paint')[0]?.startTime"
                ).json_value()
            )
            browser.close()
        yield f"cold first paint {name}", min(paints), LIMITS["cold first paint"]
        yield f"cold loaded {name}", min(loads), LIMITS["cold loaded"]


def tabs(page):
    page.goto(BOSS + "/pipeline/app")
    page.wait_for_function(rows("app"))
    times = []
    for _ in range(TRIES):
        for there in ["docs", "app"]:
            click = f"document.querySelector(\"nav.tabs a[href='/pipeline/{there}']\").click()"
            times.append(timed(page, click, rows(there)))
    yield "tab switch", min(times), LIMITS["tab switch"]


def navigation(page, pages):
    page.goto(BOSS + "/pipeline/app")
    page.wait_for_function(rows("app"))
    for name, path in pages:
        if name == "pipeline":
            continue
        link = "tbody" if name == "task" else "header"
        click = f"document.querySelector(\"{link} a[href='{path}']\").click()"
        there, back = [], []
        for _ in range(TRIES):
            there.append(timed(page, click, LOADED[name]))
            back.append(timed(page, "history.back()", LOADED["pipeline"]))
        yield f"navigate to {name}", min(there), LIMITS["navigate"]
        yield f"back from {name}", min(back), LIMITS["back"]


def main():
    binary, *modes = sys.argv[1:]
    start(binary)
    pages = [
        ("pipeline", "/pipeline/app"),
        ("task", f"/task/{a_task()}"),
        ("workers", "/workers"),
        ("settings", "/settings"),
        ("users", "/users"),
    ]
    over = 0
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        measures = {
            "cold": lambda: cold(p.chromium, pages),
            "tabs": lambda: tabs(page),
            "navigation": lambda: navigation(page, pages),
        }
        for mode in modes:
            for what, ms, limit in measures[mode]():
                print(f"{what}: {ms:.0f} ms, limit {limit} ms", flush=True)
                over += ms > limit
        browser.close()
    sys.exit(1 if over else 0)


if __name__ == "__main__":
    main()
