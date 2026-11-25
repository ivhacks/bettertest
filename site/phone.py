#!/usr/bin/env python3
"""The homepage on phones, at 320 and 390px in Chromium and WebKit: nothing scrolls sideways, no
text touches the screen's edges, every link and button is at least 44px square to a finger, and
the version in the footer is the one core/Cargo.toml says, so it cannot go stale.
"""

import tomllib
from pathlib import Path

from playwright.sync_api import sync_playwright

SITE = Path(__file__).resolve().parent
HTML = SITE / "index.html"
CARGO = tomllib.loads((SITE.parent / "core" / "Cargo.toml").read_text())
VERSION = CARGO["package"]["version"]
WIDTHS = [320, 390]
# The least room between text and either edge of the screen.
EDGE = 16
FINGER = 44

PROBLEMS = """([edge, finger]) => {
    const found = [];
    const doc = document.documentElement;
    const width = doc.clientWidth;
    if (doc.scrollWidth > width) {
        found.push(`page is ${doc.scrollWidth}px in ${width}px`);
    }
    const text = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    while (text.nextNode()) {
        const node = text.currentNode;
        if (!node.textContent.trim()) continue;
        // Text a box clips shows only as far as that box.
        let [shownLeft, shownRight] = [-Infinity, Infinity];
        for (let e = node.parentElement; e; e = e.parentElement) {
            if (getComputedStyle(e).overflowX !== 'visible') {
                const clip = e.getBoundingClientRect();
                shownLeft = Math.max(shownLeft, clip.left);
                shownRight = Math.min(shownRight, clip.right);
            }
        }
        const range = document.createRange();
        range.selectNodeContents(node);
        for (const box of range.getClientRects()) {
            const left = Math.max(box.left, shownLeft);
            const right = Math.min(box.right, shownRight);
            if (right > left && (left < edge || right > width - edge)) {
                found.push(`"${node.textContent.trim().slice(0, 30)}" shows at ` +
                    `${left.toFixed(1)}..${right.toFixed(1)}px of ${width}px`);
            }
        }
    }
    for (const target of document.querySelectorAll('a, button')) {
        const box = target.getBoundingClientRect();
        if (box.width && (box.width < finger || box.height < finger)) {
            found.push(`${target.outerHTML.slice(0, 60)} is ` +
                `${box.width.toFixed(1)}x${box.height.toFixed(1)}px`);
        }
    }
    return found;
}"""


def main() -> None:
    failures = []
    with sync_playwright() as playwright:
        for engine in (playwright.chromium, playwright.webkit):
            browser = engine.launch()
            for width in WIDTHS:
                context = browser.new_context(
                    viewport={"width": width, "height": 800},
                    is_mobile=True,
                    has_touch=True,
                )
                page = context.new_page()
                page.set_content(HTML.read_text(), wait_until="domcontentloaded")
                page.click("#try-now")
                page.wait_for_selector("#try-panel:not([hidden])")
                found = page.evaluate(PROBLEMS, [EDGE, FINGER])
                failures += [f"{engine.name} {width}px: {p}" for p in found]
                release = page.locator("footer a[href*='/releases/']")
                shown = release.inner_text().lower()
                href = release.get_attribute("href")
                if shown != f"v{VERSION}" or not href.endswith(f"/tag/v{VERSION}"):
                    failures.append(
                        f"footer says {shown} ({href}), but core/Cargo.toml is {VERSION}"
                    )
                context.close()
            browser.close()
    assert not failures, "\n".join(failures)
    print(
        f"homepage at {WIDTHS} in chromium and webkit: v{VERSION}, fits, 44px targets"
    )


if __name__ == "__main__":
    main()
