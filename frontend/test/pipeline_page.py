"""The pipeline page in a browser, with a pipeline of 41 stages (the last of them one task wide)
and a dozen tabs:

- at 320, 375, 390, 768, 1024 and 1440px, neither the page nor anything on it scrolls sideways or reaches
  past the screen, the grid and a run's results alike, and every stage name shows whole;
- a skipped task's cell is no link and does not answer the pointer;
- every other run cell lights up when hovered, turns white when pressed and fades back once
  released, like a button (brand guide, "Hover, press, release");
- run ids are muted text, not link-colored.

The boss is shell.py's stand-in: frontend/dist, plus this pipeline's snapshot. Like shell.py, it
runs in Playwright's image (see shell_nav.rs, which runs both in cargo test).
"""

import sys

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
        "pipelines": ["many_stages"] + [f"another_pipeline_{n}" for n in range(11)],
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


def main():
    # The browser side runs in Playwright's image, which has no trunk.
    if shell.dist_stale():
        raise SystemExit(
            "frontend/dist is missing or stale: run python3 test/shell.py build first"
        )
    shell.PIPELINES["many_stages"] = shell.snapshot(body())
    server, base = shell.start_server()
    from playwright.sync_api import sync_playwright

    failed = 0
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                args=["--no-sandbox", "--disable-dev-shm-usage"]
            )
            for test in [nothing_scrolls_sideways, cells_answer_like_buttons]:
                try:
                    test(browser, base)
                    print(f"PASS {test.__name__}")
                except AssertionError as error:
                    failed += 1
                    print(f"FAIL {test.__name__}\n{error}")
            browser.close()
    finally:
        server.shutdown()
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
