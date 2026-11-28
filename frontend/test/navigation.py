#!/usr/bin/env python3
"""Moving between the pages the header's icons open.

- The five icons (Pipelines, Workers, Settings, Users and the account) are one control: the same
  class, the hand cursor, the same cut-corner outline when hovered, white while pressed, and the
  `released` fade once let go.
- A page left stays up. Going back to the workers, users, settings or pipeline page paints what it
  showed before any request goes out, and its main is never empty or without that content.
- Each of those pages fits a 320px phone without anything scrolling sideways, and its tables span
  a 1280px screen.

Chromium, and WebKit as an iPhone sees it. The boss is shell.py's stand-in, with a logged-in user
(so the account is the menu's button), a worker and a user. Like shell.py, it runs in Playwright's
image (see shell_nav.rs).
"""

import json
import sys
import traceback

import shell

shell.ME = b'{"name":"iv","role":"admin","logged_in":true}'
shell.WORKERS = shell.snapshot(
    {
        "now": 10_000,
        "workers": [
            {
                "url": "http://gpu-rig-01:7358",
                "info": {
                    "hostname": "gpu-rig-01",
                    "arch": "x86_64",
                    "version": "0.2.0",
                    "labels": ["x86", "gpu"],
                    "capacity": 4,
                },
                "since": 1_000,
                "problem": None,
                "tasks": [],
            }
        ],
    }
)
USER = {
    "id": 1,
    "name": "alice",
    "role": "write",
    "builtin": False,
    "passwords": 1,
    "certificates": 0,
    "passkeys": 0,
    "tokens": 0,
    "certs": [],
    "keys": [],
}
shell.USERS = json.dumps([USER]).encode()

ICONS = ["Pipelines", "Workers", "Settings", "Users", "Account, iv"]

# Each page: the icon that opens it, where it lands, and what only it shows once it has its data.
PAGES = {
    "workers": ("Workers", "/workers", "table.workers td.url"),
    "users": ("Users", "/users", "table.users td.name"),
    "settings": ("Settings", "/settings", "form.settings input"),
    "pipelines": ("Pipelines", "/pipeline/app", f".grid a[href='/task/{shell.TASK}']"),
}

# Pages left stay in the document, hidden: only what shows counts.
SHOWN = "(s) => [...document.querySelectorAll(s)].find((e) => e.checkVisibility())"

# How an icon looks: everything the hover outline, the cursor and the cut corners are made of.
LOOK = """(icon) => {
    const css = getComputedStyle(icon);
    const before = getComputedStyle(icon, "::before");
    return {
        cursor: css.cursor,
        background: css.backgroundColor,
        color: css.color,
        clip: css.clipPath,
        before: [before.content, before.backgroundColor, before.inset, before.clipPath].join(" "),
        size: [icon.offsetWidth, icon.offsetHeight],
    };
}"""


def icon(page, name):
    return page.locator(f'header .links > [aria-label="{name}"]:visible')


def test_header_icons_are_one_control(browser, base, device):
    context = browser.new_context(**device, color_scheme="dark")
    page = context.new_page()
    try:
        page.goto(base + "/pipeline/app")
        page.wait_for_selector(PAGES["pipelines"][2])
        # Pressed and released, an icon goes nowhere: what it looks like is what is under test.
        page.evaluate(
            "addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); }, true)"
        )
        controls = page.locator("header:visible .links > :is(a, button)")
        names = [
            controls.nth(i).get_attribute("aria-label") for i in range(controls.count())
        ]
        assert names == ICONS, names
        failures = []
        classes = {name: icon(page, name).get_attribute("class") for name in ICONS}
        if set(classes.values()) != {"icon"}:
            failures.append(f"not one class: {classes}")
        looks = {}
        for name in ICONS:
            control = icon(page, name)
            box = control.bounding_box()
            middle = (box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.move(0, 0)
            resting = control.evaluate(LOOK)
            page.mouse.move(*middle)
            hovered = control.evaluate(LOOK)
            page.mouse.down()
            pressed = control.evaluate(LOOK)
            page.mouse.up()
            released = control.evaluate(
                "(icon) => [icon.classList.contains('released'), getComputedStyle(icon).animationName]"
            )
            looks[name] = (resting, hovered, pressed["background"], released)
            if resting["size"] != [44, 44]:
                failures.append(f"{name}: not 44px square: {resting}")
            if hovered["cursor"] != "pointer":
                failures.append(f"{name}: no hand cursor: {hovered}")
            if hovered["background"] == resting["background"]:
                failures.append(f"{name}: no hover outline: {hovered}")
            if pressed["background"] != "rgb(255, 255, 255)":
                failures.append(f"{name}: not white while pressed: {pressed}")
            if released != [True, "released"]:
                failures.append(f"{name}: no release fade: {released}")
        first = looks[ICONS[0]]
        for name, look in looks.items():
            if look != first:
                failures.append(f"{name} looks unlike {ICONS[0]}:\n  {look}\n  {first}")
        assert not failures, "\n".join(failures)
    finally:
        context.close()


ARM = (
    """([path, selector]) => {
    const shown = %s;
    const ready = () => location.pathname === path && !!shown(selector);
    const seen = (window.__revisit = { requests: [], blank: false, ready: false, started: false });
    const note = (url) => seen.requests.push({ url: String(url), ready: ready() });
    const fetch = window.fetch;
    window.fetch = function (input, init) {
        note(input.url ?? input);
        return fetch.apply(this, arguments);
    };
    const Source = window.EventSource;
    window.EventSource = class extends Source {
        constructor(url, options) {
            note(url);
            super(url, options);
        }
    };
    addEventListener("pointerdown", () => { seen.started = true; }, { capture: true, once: true });
    new MutationObserver(() => {
        if (!seen.started || location.pathname !== path) return;
        if (ready()) seen.ready = true;
        else seen.blank = true;
    }).observe(document.documentElement, { childList: true, subtree: true, attributes: true });
}"""
    % SHOWN
)


def visit(page, name):
    label, path, selector = PAGES[name]
    icon(page, label).click()
    page.wait_for_function(
        f"() => location.pathname === {path!r} && ({SHOWN})({selector!r})"
    )


def test_a_page_comes_back_before_any_request(browser, base, device):
    context = browser.new_context(**device, color_scheme="dark")
    page = context.new_page()
    try:
        page.goto(base + "/pipeline/app")
        page.wait_for_selector(PAGES["pipelines"][2])
        # First visits, each loaded in full before the next.
        for name in ["workers", "users", "settings"]:
            visit(page, name)
            page.wait_for_timeout(300)
        failures = []
        for name in ["workers", "pipelines", "users", "workers", "settings"]:
            _, path, selector = PAGES[name]
            page.evaluate(ARM, [path, selector])
            visit(page, name)
            page.wait_for_timeout(300)
            seen = page.evaluate("window.__revisit")
            if seen["blank"] or not seen["ready"]:
                failures.append(f"{name}: on screen without its content: {seen}")
            early = [r["url"] for r in seen["requests"] if not r["ready"]]
            if early:
                failures.append(
                    f"{name}: asked the boss before painting what it had: {early}"
                )
            # The icon pressed to leave this page last time stays in it, hidden. It must not fade
            # back from white again now that it shows.
            fading = page.evaluate(
                "() => [...document.querySelectorAll('.released')].filter((e) => e.checkVisibility()).map((e) => e.outerHTML.slice(0, 60))"
            )
            if fading:
                failures.append(
                    f"{name}: fades a button released on an earlier visit: {fading}"
                )
        assert not failures, "\n".join(failures)
    finally:
        context.close()


# Anything on the page that scrolls sideways: the page itself, or a region of it.
SIDEWAYS = """() => [document.scrollingElement, ...document.querySelectorAll('body *')]
    .filter((e) => e.checkVisibility() && e.scrollWidth > e.clientWidth
        && (e === document.scrollingElement || ['auto', 'scroll'].includes(getComputedStyle(e).overflowX)))
    .map((e) => `${e.tagName.toLowerCase()}.${e.className} is ${e.scrollWidth}px in ${e.clientWidth}px`)"""

# How much of main's width each table's rows take (a table made a block is wide, its rows not).
# The run grid is a CSS grid, held to the width by pipeline_page.py.
FILLED = """() => [...document.querySelectorAll('main table:not(.grid)')].filter((t) => t.checkVisibility()).map((t) => {
    const main = getComputedStyle(t.closest('main'));
    const room = t.closest('main').clientWidth - parseFloat(main.paddingLeft) - parseFloat(main.paddingRight);
    const widest = Math.max(...[...t.rows].map((row) => row.getBoundingClientRect().width));
    return [t.className, widest / room];
})"""


def test_pages_fit_and_fill_the_width(browser, base, device):
    """From a phone to a desktop, no page scrolls sideways, any part of it either, and on a wide
    screen the tables span the page rather than float in a corner of it."""
    failures = []
    for width in [320, 390, 1280]:
        context = browser.new_context(
            **{**device, "viewport": {"width": width, "height": 800}}
        )
        page = context.new_page()
        for name, (_, path, selector) in PAGES.items():
            page.goto(base + path)
            page.wait_for_selector(selector)
            failures += [f"{width}px {name}: {s}" for s in page.evaluate(SIDEWAYS)]
            if width > 640:
                failures += [
                    f"{width}px {name}: table.{table} takes {share:.0%} of the width"
                    for table, share in page.evaluate(FILLED)
                    if share < 0.99
                ]
        context.close()
    assert not failures, "\n".join(failures)


def main():
    if shell.dist_stale():
        raise SystemExit(
            "frontend/dist is missing or older than frontend/: run ./build.sh (or frontend/build.sh) first"
        )
    server, base = shell.start_server()
    from playwright.sync_api import sync_playwright

    failed = 0
    try:
        with sync_playwright() as playwright:
            chromium = playwright.chromium.launch(
                args=["--no-sandbox", "--disable-dev-shm-usage"]
            )
            webkit = playwright.webkit.launch()
            desktop = {"viewport": {"width": 1280, "height": 800}}
            # An iPhone's screen, but a mouse: hover is what the test is about.
            iphone = {
                "viewport": {"width": 390, "height": 800},
                "user_agent": playwright.devices["iPhone 15"]["user_agent"],
            }
            try:
                tests = [
                    test_header_icons_are_one_control,
                    test_a_page_comes_back_before_any_request,
                    test_pages_fit_and_fill_the_width,
                ]
                for test in tests:
                    for engine, device in [("chromium", desktop), ("webkit", iphone)]:
                        browser = chromium if engine == "chromium" else webkit
                        try:
                            test(browser, base, device)
                            print(f"PASS {test.__name__} {engine}")
                        except Exception:
                            failed += 1
                            print(f"FAIL {test.__name__} {engine}")
                            traceback.print_exc()
            finally:
                webkit.close()
                chromium.close()
    finally:
        server.shutdown()
    if failed:
        sys.exit(f"{failed} navigation test(s) failed")


if __name__ == "__main__":
    main()
