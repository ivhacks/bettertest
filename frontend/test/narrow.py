"""Every page at phone widths: nothing makes the page scroll sideways, and every stage name in the
grid shows whole. Needs a boss and worker running long_names.py, and Playwright's Chromium:

    bettertest unified frontend/test/long_names.py
    python3 frontend/test/narrow.py [http://[::1]:7357]

It starts a run of long_names, waits for it to end, and checks each page at 320 and 390px.
"""

import json
import sys
import urllib.request

from playwright.sync_api import Page, sync_playwright

BOSS = sys.argv[1] if len(sys.argv) > 1 else "http://[::1]:7357"
WIDTHS = [320, 390]
STAGES = 6

PROBLEMS = """() => {
    const problems = [];
    const page = document.documentElement;
    if (page.scrollWidth > page.clientWidth) {
        problems.push(`the page is ${page.scrollWidth}px wide`);
    }
    const main = document.querySelector('main');
    if (main.scrollWidth > main.clientWidth) {
        problems.push(`main scrolls sideways: ${main.scrollWidth}px in ${main.clientWidth}px`);
    }
    for (const name of document.querySelectorAll('.grid th.stage span')) {
        if (name.scrollWidth > name.clientWidth) {
            problems.push(`stage ${name.textContent} is cut off`);
        }
    }
    return problems;
}"""


def api(path: str, method: str = "GET"):
    request = urllib.request.Request(BOSS + path, method=method)
    with urllib.request.urlopen(request) as response:
        return json.load(response)


def settle(page: Page, path: str) -> None:
    page.goto(BOSS + path)
    page.wait_for_selector("header")
    page.evaluate("document.fonts.ready")
    # The grid measures its names after rendering, then renders again.
    page.wait_for_timeout(300)


def main() -> None:
    run = api("/api/start-run/long_names", "POST")
    task = next(
        t
        for s in run["stages"]
        for t in s["tasks"]
        if t["name"] == "compile_frontend_bundle"
    )
    pages = [f"/pipeline/{name}" for name in api("/api/pipelines")]
    pages += [
        f"/pipeline/long_names?run={run['id']}",
        f"/task/{task['id']}",
        "/workers",
        "/settings",
        "/users",
        "/login",
    ]
    failures = []
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page()
        settle(page, "/pipeline/long_names")
        page.wait_for_selector("th.run.running", state="detached", timeout=120_000)
        for width in WIDTHS:
            phone = browser.new_context(
                viewport={"width": width, "height": 800}, is_mobile=True, has_touch=True
            )
            page = phone.new_page()
            for path in pages:
                settle(page, path)
                failures += [f"{width}px {path}: {p}" for p in page.evaluate(PROBLEMS)]
            settle(page, "/pipeline/long_names")
            stages = page.locator(".grid th.stage span").count()
            if stages != STAGES:
                failures.append(
                    f"{width}px: {stages} stage names on long_names, not {STAGES}"
                )
            phone.close()
        browser.close()
    for failure in failures:
        print(failure)
    print(
        f"{len(pages)} pages at {', '.join(f'{w}px' for w in WIDTHS)}: {len(failures)} problems"
    )
    sys.exit(1 if failures else 0)


main()
