import re
import subprocess
from pathlib import Path

from bettertest import (
    Labels,
    Stage,
    artifact,
    copy,
    embed,
    get_artifact_this_run,
    log,
    run,
    task,
    tests,
    view,
)


class Label(Labels):
    x86 = "x86"


PYTHON = "python:3.13-slim"
# Its browsers are the ones @playwright/test of the same version drives.
PLAYWRIGHT = "mcr.microsoft.com/playwright:v1.65.1-noble"

APP = '''
import re


def slug(title):
    """'Hello, World!' -> 'hello-world'"""
    return re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
'''

MAIN = """
import sys

from app import slug

print(slug(" ".join(sys.argv[1:])))
"""

UNIT = """
import unittest

from app import slug

CASES = {
    "Hello, World!": "hello-world", "  spaces  ": "spaces", "a--b": "a-b", "": "",
    "CamelCase": "camelcase", "snake_case": "snake-case", "x86_64": "x86-64", "1.2.3": "1-2-3",
    "émoji 🎉": "moji", "tabs\\tand\\nlines": "tabs-and-lines", "---": "", "UPPER": "upper",
}


class Slug(unittest.TestCase):
    pass


for i, (title, expected) in enumerate(CASES.items()):
    setattr(Slug, f"test_{i:02}", lambda self, t=title, e=expected: self.assertEqual(slug(t), e))
"""

E2E = """
const { test, expect } = require("@playwright/test");

const PAGE = `<input id="title" placeholder="title"><output id="slug"></output>
<script>
  title.oninput = () => slug.value = title.value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
</script>`;

test("typing a title shows its slug", async ({ page }) => {
  await page.setContent(PAGE);
  await page.fill("#title", "Hello, World!");
  await expect(page.locator("#slug")).toHaveText("hello-world");
});

test("an empty title has an empty slug", async ({ page }) => {
  await page.setContent(PAGE);
  await page.fill("#title", "!!!");
  await expect(page.locator("#slug")).toHaveText("");
});

test("the field has a placeholder", async ({ page }) => {
  await page.setContent(PAGE);
  await expect(page.locator("#title")).toHaveAttribute("placeholder", "title");
});
"""

# The results page frames an HTML artifact in a sandbox, an origin of its own so that it cannot act
# as whoever looks at it. Such an origin has no localStorage, where the report keeps its settings:
# it gets one that lasts as long as the page.
MEMORY = r"""
const fs = require("fs");
const file = process.argv[2];
const memory = `<script>try { localStorage } catch { const kept = new Map(); Object.defineProperty(window, "localStorage", { value: { getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, String(v)), removeItem: (k) => kept.delete(k), clear: () => kept.clear(), key: (i) => [...kept.keys()][i] ?? null, get length() { return kept.size; } } }); }</script>`;
fs.writeFileSync(file, fs.readFileSync(file, "utf8").replace("<head>", "<head>" + memory));
"""

CONFIG = """
module.exports = {
  reporter: [["html", { open: "never" }], ["json", { outputFile: "results.json" }], ["list"]],
  use: { trace: "off", screenshot: "off" },
};
"""


class Build(Stage):
    labels = (Label.x86,)

    @task(image=PYTHON)
    def zipapp():
        Path("app.py").write_text(APP)
        Path("__main__.py").write_text(MAIN)
        run("mkdir /build")
        copy("app.py", "/build/app.py")
        copy("__main__.py", "/build/__main__.py")
        run("python -m zipapp /build -o /app.pyz && ls -l /app.pyz")
        artifact("/app.pyz", name="app.pyz")


class Test(Stage):
    labels = (Label.x86,)

    @task(image=PYTHON)
    def unit():
        Path("test_slug.py").write_text(UNIT)
        copy(get_artifact_this_run(Build.zipapp, name="app.pyz"), "/app.pyz")
        copy("test_slug.py", "/test_slug.py")
        # A zipapp is importable as it is. Failing tests are counted below, not fatal here.
        out = run(
            "cd / && PYTHONPATH=/app.pyz python -m unittest -v test_slug 2>&1 || true"
        )
        summary = re.search(r"Ran (\d+) test", out)
        if not summary:
            raise SystemExit("unittest stopped before running the tests")
        ran = int(summary.group(1))
        failed = sum(int(n) for n in re.findall(r"(?:failures|errors)=(\d+)", out))
        tests(total=ran, failed=failed)
        if failed:
            raise SystemExit(f"{failed} of {ran} unit tests failed")

    @task()
    def smoke():
        app = get_artifact_this_run(Build.zipapp, name="app.pyz")
        slug = subprocess.run(
            ["python3", app, "Smoke", "Test!"],
            check=False,
            capture_output=True,
            text=True,
        )
        print(f"python3 app.pyz 'Smoke Test!' -> {slug.stdout.strip()!r}")
        tests(total=1, failed=int(slug.stdout.strip() != "smoke-test"))
        assert slug.stdout.strip() == "smoke-test", slug

    @task(image=PLAYWRIGHT, timeout=600)
    def e2e():
        files = {
            "e2e.spec.js": E2E,
            "playwright.config.js": CONFIG,
            "memory.js": MEMORY,
        }
        run("mkdir /e2e")
        for name, text in files.items():
            Path(name).write_text(text)
            copy(name, f"/e2e/{name}")
        run(
            "cd /e2e && npm install --no-save --no-audit --no-fund @playwright/test@1.65.1"
        )
        # Failures still leave a report to publish, so this goes on either way.
        run("cd /e2e && npx playwright test 2>&1 || true")
        run("node /e2e/memory.js /e2e/playwright-report/index.html")
        artifact("/e2e/playwright-report/index.html", name="playwright-report.html")
        stats = "const s = require('/e2e/results.json').stats; [s.expected + s.unexpected + s.flaky, s.unexpected].join(' ')"
        total, failed = map(int, run(f'node -p "{stats}"').split())
        tests(total=total, failed=failed)
        if failed:
            raise SystemExit(f"{failed} of {total} browser tests failed")


# The views: what each run's results page shows above its tasks, drawn from that run.

COLORS = {"pass": "#2ea44f", "fail": "#e5484d", "running": "#ff6e00"}


def tasks(run):
    return [(f"{s['name']}.{t['name']}", t) for s in run["stages"] for t in s["tasks"]]


@view()
def tests_by_task(run):
    counted = [(name, t["tests"]["total"]) for name, t in tasks(run) if t["tests"]]
    pie = {
        "type": "pie",
        "labels": [name for name, _ in counted],
        "values": [total for _, total in counted],
        "hole": 0.55,
        "sort": False,
        "textinfo": "label+value",
        "textposition": "outside",
    }
    return {
        "data": [pie],
        "layout": {"showlegend": False, "margin": {"t": 40, "b": 40}},
    }


@view()
def seconds_per_task(run):
    timed = [(name, t) for name, t in tasks(run) if t["started"] and t["ended"]]
    bar = {
        "type": "bar",
        "x": [name for name, _ in timed],
        "y": [(t["ended"] - t["started"]) / 1000 for _, t in timed],
        "marker": {"color": [COLORS.get(t["state"], "#8a8a8a") for _, t in timed]},
        "hovertemplate": "%{x}: %{y:.1f}s<extra></extra>",
    }
    return {"data": [bar], "layout": {"yaxis": {"ticksuffix": "s"}}}


@view()
def tests_passed(run):
    counts = [t["tests"] for _, t in tasks(run) if t["tests"]]
    total = sum(c["total"] for c in counts)
    passed = total - sum(c["failed"] for c in counts)
    number = {
        "type": "indicator",
        "mode": "number",
        "value": passed,
        "number": {"suffix": f" / {total}", "font": {"size": 72}},
    }
    color = COLORS["pass" if passed == total else "fail"]
    return {"data": [number], "layout": {"font": {"color": color}}}


@view(wide=True)
def unit_and_e2e_logs(run):
    columns = []
    for i, shown in enumerate([Test.unit, Test.e2e]):
        lines = log(shown).splitlines() or ["(nothing yet)"]
        columns.append(
            {
                "type": "table",
                "domain": {"x": [i * 0.51, i * 0.51 + 0.49]},
                "columnwidth": [1],
                "header": {"values": [shown.__qualname__], "height": 28},
                "cells": {"values": [lines], "height": 22},
            }
        )
    return {"data": columns, "layout": {"height": 440}}


@view(wide=True)
def playwright_report(run):
    return embed(Test.e2e, name="playwright-report.html")
