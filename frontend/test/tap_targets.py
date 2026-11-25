"""Every page on a phone, at 320 and 390px in Chromium and WebKit: each link, button and field takes
a finger, at least 44 by 44px of hit area, and nothing scrolls sideways.

Hit area is what the browser hit-tests, not the element's box: how many whole pixels across and
down, in one unbroken run through the target, a tap lands on it. So padding and pseudo-elements
count, and a neighbour drawn over it does not. A grid cell's column can be narrower than a finger
(there is no room for more tasks across a phone without scrolling sideways), so a cell's hit area
is 44px tall and its whole column wide, gap included, and 44px wide wherever the column is.

The boss is shell.py's stand-in, with pipeline_page.py's pipeline of 41 stages beside shell.py's
one-task pipelines. Like shell.py, it runs in Playwright's image (see shell_nav.rs).
"""

import sys

import pipeline_page
import shell

WIDTHS = [320, 390]
FINGER = 44

# For each visible target: its hit area, and how wide its grid column is (null outside the grid).
HIT_AREAS = """(finger) => {
    const found = [];
    const targets = document.querySelectorAll('a, button, input, select, textarea');
    for (const target of targets) {
        const shown = target.getBoundingClientRect();
        if (!shown.width || !shown.height || getComputedStyle(target).visibility === 'hidden') continue;
        const cell = target.closest('.grid td');
        let box = (cell || target).getBoundingClientRect();
        // main scrolls under the header and above the tabs.
        const view = target.closest('main')?.getBoundingClientRect() ?? {top: 0, bottom: innerHeight};
        if (box.top < view.top + finger || box.bottom > view.bottom - finger) {
            target.scrollIntoView({block: 'center'});
            box = (cell || target).getBoundingClientRect();
        }
        const lands = (x, y) => target.contains(document.elementFromPoint(x, y));
        // The unbroken run of whole pixels, along a line through the box, where a tap lands on it:
        // how many, and the one in the middle.
        const along = (from, to, at) => {
            let first = Math.floor(from);
            while (first <= to && !at(first)) first++;
            if (first > to) return [0, null];
            let [low, high] = [first, first];
            while (high - first < 2 * finger && at(high + 1)) high++;
            while (first - low < 2 * finger && at(low - 1)) low--;
            return [high - low + 1, Math.floor((low + high) / 2)];
        };
        const y = Math.round(box.top + box.height / 2);
        const [width, x] = along(box.left, box.right, x => lands(x, y));
        const [height] = x === null ? [0] : along(box.top, box.bottom, y => lands(x, y));
        found.push({
            target: target.outerHTML.slice(0, 80),
            width,
            height,
            column: cell ? box.width : null,
        });
    }
    return found;
}"""


# The page, or anything on it that scrolls, is wider than the screen. (Plotly parks a measuring svg
# far off the left edge, which pipeline_page.py's stricter check, for pages without views, would
# count.)
SIDEWAYS = """() => {
    const found = [];
    for (const e of [document.scrollingElement, ...document.querySelectorAll('body *')]) {
        const scrolls = e === document.scrollingElement ||
            ['auto', 'scroll'].includes(getComputedStyle(e).overflowX);
        if (scrolls && e.scrollWidth > e.clientWidth) {
            found.push(`${e.tagName.toLowerCase()} scrolls sideways: ${e.scrollWidth}px in ${e.clientWidth}px`);
        }
    }
    return found;
}"""


def pages():
    task = shell.TASK
    many = pipeline_page.run_id(3)
    return [
        ("/pipeline/app", ".grid tbody td"),
        ("/pipeline/many_stages", ".grid tbody td"),
        (f"/pipeline/app?run={shell.RUN}", ".results"),
        (f"/pipeline/many_stages?run={many}", ".results"),
        (f"/task/{task}", ".log"),
        ("/workers", "main"),
        ("/settings", "input"),
        ("/users", "main"),
        ("/login", "input"),
    ]


def too_small(area):
    # A hit area is whole pixels found one at a time; a box of 43.6px gives 43 of them.
    least = FINGER - 1
    across = least if area["column"] is None else min(least, int(area["column"]) - 1)
    if across <= 0:
        # A column under 2px has no pixel of its own: its neighbours' cells take its taps.
        return False
    return area["width"] < across or area["height"] < least


def tap_targets_take_a_finger(browsers, base):
    failures = []
    for name, browser in browsers:
        for width in WIDTHS:
            context = browser.new_context(
                viewport={"width": width, "height": 800}, is_mobile=True, has_touch=True
            )
            page = context.new_page()
            for path, ready in pages():
                pipeline_page.settle(page, base + path, ready)
                where = f"{name} {width}px {path}"
                failures += [f"{where}: {p}" for p in page.evaluate(SIDEWAYS)]
                areas = page.evaluate(HIT_AREAS, FINGER)
                if not areas:
                    failures.append(f"{where}: no targets")
                for area in filter(too_small, areas):
                    column = (
                        ""
                        if area["column"] is None
                        else f" in a {area['column']:.1f}px column"
                    )
                    failures.append(
                        f"{where}: {area['target']} takes {area['width']}x{area['height']}px{column}"
                    )
            context.close()
    assert not failures, "\n".join(failures)


def main():
    if shell.dist_stale():
        raise SystemExit(
            "frontend/dist is missing or older than frontend/: run ./build.sh (or frontend/build.sh) first"
        )
    shell.PIPELINES["many_stages"] = shell.snapshot(pipeline_page.body())
    server, base = shell.start_server()
    from playwright.sync_api import sync_playwright

    try:
        with sync_playwright() as playwright:
            chromium = playwright.chromium.launch(
                args=["--no-sandbox", "--disable-dev-shm-usage"]
            )
            webkit = playwright.webkit.launch()
            try:
                tap_targets_take_a_finger(
                    [("chromium", chromium), ("webkit", webkit)], base
                )
                print("PASS tap_targets_take_a_finger")
            except AssertionError as error:
                print(f"FAIL tap_targets_take_a_finger\n{error}")
                sys.exit(1)
            finally:
                webkit.close()
                chromium.close()
    finally:
        server.shutdown()


if __name__ == "__main__":
    main()
