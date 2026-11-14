#!/usr/bin/env python3
"""The shell stays put, and the first document stays inside one burst.

Nathaniel's rule of thumb (endtimes.dev, "Why your website should be under 14kB"): a new
connection sends about 10 TCP packets first, 10 x 1460 = 14600 bytes on the wire, then waits.
That budget is the HTML document, headers included: the chrome. The first visit also downloads
the wasm and, on top of that, Syeve (the AI; issues #6 and #7). Those are larger than the burst
and the document check does not fail for them. What it does demand:

- the document's own transferSize, headers included, fits in the burst
- a first visit that also downloads Syeve still passes that document check
- in-app navigation does not re-download the shell (wasm, JS, CSS, fonts) or any other large body
- back and forward restore the URL and the visible page without a shell reload
- a repeat load takes large assets from cache instead of transferring them again
- a large text/wasm/css/js response is actually compressed
- the same large asset is not fetched twice
- WebKit, as an iPhone, starts the app over TLS as the boss speaks it, on a first visit and on a
  second whose connections resume the TLS session

The server is local and speaks the same cache and gzip rules as the boss's embedded files.
It never talks to a live bettertest. Syeve is not in the product yet, so this file serves a
stand-in payload and the size test requests it after the frame is up.
"""

import gzip
import json
import os
import ssl
import subprocess
import sys
import tempfile
import threading
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
# 10 packets x 1460 bytes of payload, the first burst the article is about.
FIRST_BURST = 10 * 1460
# A body above this that is text, script, style or wasm and is not actually smaller on the wire
# is the waste the test is here to catch.
SHOULD_COMPRESS = 2048
TASK = "11111111-1111-1111-1111-111111111111"
OTHER = "22222222-2222-2222-2222-222222222222"
RUN = "6dfde700-0000-0000-0000-000000000001"
SHELL_SUFFIXES = (".wasm", ".js", ".css", ".woff2")
# Stand-in for Syeve. Incompressible, and larger than the burst, so the size test has really
# seen a first visit that also downloaded the AI. The product does not serve this yet.
SYEVE_PATH = "/syeve/model.bin"
SYEVE_BODY = os.urandom(20_000)

# Filled by the size test and printed at the end, so the run leaves real numbers behind.
MEASUREMENTS = {}
# Over TLS, whether each request came on a resumed session.
RESUMED = []
# What browsers that could not start the app reported to /api/startup-error.
STARTUP_ERRORS = []
# While set, the wasm is served corrupt, so the app cannot start.
BREAK_WASM = threading.Event()


def accepts_gzip(header):
    """Same rule as accepts_gzip in core/src/boss/mod.rs."""
    for part in header.split(","):
        params = [p.strip() for p in part.split(";")]
        if not params or params[0].lower() != "gzip":
            continue
        refused = False
        for param in params[1:]:
            name, _, value = param.partition("=")
            if name.strip().lower() != "q":
                continue
            try:
                if float(value.strip()) <= 0:
                    refused = True
            except ValueError:
                pass
        if not refused:
            return True
    return False


def compressible(path):
    """Same set as compressible() in core/src/boss/mod.rs. SVG is excluded: Chrome refetches a
    gzipped SVG image on the next load."""
    ext = path.rsplit(".", 1)[-1]
    return ext in {"html", "css", "js", "wasm", "json", "txt", "map"}


def cache_control(path):
    if path == "index.html":
        return "no-cache"
    return "public, max-age=31536000, immutable"


def content_type(path):
    return {
        ".html": "text/html",
        ".css": "text/css",
        ".js": "text/javascript",
        ".wasm": "application/wasm",
        ".svg": "image/svg+xml",
        ".png": "image/png",
        ".woff2": "font/woff2",
        ".txt": "text/plain",
        ".json": "application/json",
    }.get(Path(path).suffix, "application/octet-stream")


def task_run(task_id, name):
    return {
        "id": task_id,
        "name": name,
        "state": "pass",
        "worker": "http://worker:7358",
        "started": 1_000,
        "ended": 2_500,
        "carried_from": None,
        "tests": None,
    }


def pipeline_body(name, task_name, task_id):
    task = {
        "name": task_name,
        "worker": None,
        "labels": ["x86"],
        "image": None,
        "vm": None,
        "arch": None,
        "timeout": None,
        "sandbox": None,
    }
    run = {
        "id": RUN,
        "pipeline": name,
        "state": "pass",
        "commit": None,
        "stages": [{"name": "Test", "tasks": [task_run(task_id, task_name)]}],
    }
    return {
        "name": name,
        "pipelines": ["app", "docs"],
        "pipeline": {
            "Ok": {
                "stages": [{"name": "Test", "sandbox": None, "tasks": [task]}],
                "sandboxes": {},
            }
        },
        "runs": [run],
    }


def task_body(task_id, name, pipeline):
    run = {
        "id": RUN,
        "pipeline": pipeline,
        "state": "pass",
        "commit": None,
        "stages": [{"name": "Test", "tasks": [task_run(task_id, name)]}],
    }
    return {
        "now": 10_000,
        "run": run,
        "carried_from": None,
        "log_deleted": None,
        "artifacts_deleted": None,
        "stage": "Test",
        "task": task_run(task_id, name),
        "lines": [{"stream": "stdout", "line": "shell-nav-marker"}],
        "artifacts": [],
    }


def snapshot(body):
    return json.dumps({"Snapshot": body}).encode()


PIPELINES = {
    "app": snapshot(pipeline_body("app", "unit", TASK)),
    "docs": snapshot(pipeline_body("docs", "spell", OTHER)),
}
TASKS = {
    TASK: snapshot(task_body(TASK, "unit", "app")),
    OTHER: snapshot(task_body(OTHER, "spell", "docs")),
}
WORKERS = snapshot({"now": 10_000, "workers": []})


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("shell-server " + (fmt % args) + "\n")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if urlparse(self.path).path == "/api/startup-error":
            STARTUP_ERRORS.append(body.decode("utf-8", "replace"))
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", "2")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(b"{}")

    def do_GET(self):
        if isinstance(self.connection, ssl.SSLSocket):
            RESUMED.append(self.connection.session_reused)
        path = urlparse(self.path).path
        if path == "/api/me":
            self._json(b'{"name":"logged out","role":"admin","logged_in":false}')
            return
        if path == "/api/pipelines":
            self._json(b'["app","docs"]')
            return
        if path == "/api/settings":
            self._json(b'{"artifact_days":null,"log_days":null}')
            return
        if path == "/api/https":
            self._json(b"null")
            return
        if path.startswith("/api/pipeline/") and path.endswith("/events"):
            name = path.split("/")[3]
            self._sse(PIPELINES.get(name, PIPELINES["app"]))
            return
        if path.startswith("/api/task/") and path.endswith("/events"):
            task = path.split("/")[3]
            body = TASKS.get(task)
            if body is None:
                self.send_error(404)
                return
            self._sse(body)
            return
        if path == "/api/workers/events":
            self._sse(WORKERS)
            return
        if path == SYEVE_PATH:
            # Background AI payload. Immutable like any other big file, and not the document.
            self._raw(SYEVE_BODY, "application/octet-stream", "public, max-age=31536000, immutable")
            return
        if self._app_page(path):
            self._file("index.html")
            return
        rel = path.lstrip("/")
        if not rel or ".." in rel.split("/"):
            self.send_error(404)
            return
        file = (DIST / rel).resolve()
        if not str(file).startswith(str(DIST.resolve())) or not file.is_file():
            self.send_error(404)
            return
        self._file(rel)

    def _app_page(self, path):
        if path in {"/", "/workers", "/settings", "/users", "/login"}:
            return True
        return path.startswith("/pipeline/") or path.startswith("/task/")

    def _raw(self, body, mime, cache):
        self.send_response(200)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", cache)
        self.end_headers()
        self.wfile.write(body)

    def _json(self, body):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _sse(self, body):
        # One snapshot, then silence the client can hold. A closed stream would reconnect.
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        try:
            self.wfile.write(b"event: message\ndata: " + body + b"\n\n")
            self.wfile.flush()
            while True:
                threading.Event().wait(15)
                self.wfile.write(b"event: ping\ndata: \n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            return

    def _file(self, rel):
        data = (DIST / rel).read_bytes()
        if rel.endswith(".wasm") and BREAK_WASM.is_set():
            data = b"not wasm" + data
        accept = self.headers.get("Accept-Encoding", "")
        encoding = None
        if compressible(rel) and accepts_gzip(accept):
            compressed = gzip.compress(data, compresslevel=9)
            if len(compressed) < len(data):
                data = compressed
                encoding = "gzip"
        self.send_response(200)
        self.send_header("Content-Type", content_type(rel))
        self.send_header("Cache-Control", cache_control(rel))
        self.send_header("Content-Length", str(len(data)))
        if compressible(rel):
            self.send_header("Vary", "accept-encoding")
        if encoding:
            self.send_header("Content-Encoding", encoding)
        self.end_headers()
        self.wfile.write(data)


def dist_stale():
    index = DIST / "index.html"
    if not index.exists():
        return True
    built = index.stat().st_mtime
    watched = [ROOT / "index.html", ROOT / "style.css", ROOT / "Cargo.toml"]
    for folder in (ROOT / "src", ROOT / "assets"):
        watched.extend(p for p in folder.rglob("*") if p.is_file())
    return any(path.stat().st_mtime > built for path in watched if path.exists())


def build_dist():
    if dist_stale():
        subprocess.check_call(["trunk", "build", "--release"], cwd=ROOT)
    if not (DIST / "index.html").is_file():
        raise SystemExit("trunk build did not produce dist/index.html")


def start_server(tls=False):
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    if tls:
        server.socket = tls_context().wrap_socket(
            server.socket, server_side=True, do_handshake_on_connect=False
        )
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    return server, f"{'https' if tls else 'http'}://{host}:{port}"


def tls_context():
    """TLS as the boss speaks it: HTTP/1.1, a client certificate asked for without insisting,
    and sessions that resume (Python sets the session id context OpenSSL needs for that)."""
    folder = Path(tempfile.mkdtemp())
    cert, key = folder / "boss.crt", folder / "boss.key"
    subprocess.check_call(
        ["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256"]
        + ["-nodes", "-days", "1", "-subj", "/CN=localhost", "-keyout", key, "-out", cert],
        stderr=subprocess.DEVNULL,
    )
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    context.load_verify_locations(cert)
    context.verify_mode = ssl.CERT_OPTIONAL
    return context


def shell_path(url):
    path = urlparse(url).path
    # Syeve is the background AI, not the chrome. A navigation that transfers it is still a
    # large uncached body and fails that check separately.
    if "syeve" in path.lower():
        return False
    return path.endswith(SHELL_SUFFIXES) or path.startswith("/assets/")


def code_path(url):
    path = urlparse(url).path
    return path.endswith((".wasm", ".js", ".css", ".html"))


class Page:
    def __init__(self, browser, base):
        self.base = base
        self.context = browser.new_context()
        self.page = self.context.new_page()
        self.page.set_default_timeout(30_000)
        self.responses = []
        self.page.on("response", lambda response: self.responses.append(response))

    def close(self):
        self.context.close()

    def goto_app(self):
        self.page.goto(self.base + "/pipeline/app", wait_until="domcontentloaded")
        self.page.wait_for_selector(f"a[href='/task/{TASK}']")
        self.page.evaluate("() => { window.__btShell = 1 }")

    def shell_alive(self):
        return self.page.evaluate("() => window.__btShell") == 1

    def resources_since_clear(self):
        # startTime, not only clearResourceTimings: a fetch that began on the previous page can
        # land in the buffer after the clear, and that is not this navigation.
        self.page.evaluate(
            """() => {
                window.__btSince = performance.now();
                performance.clearResourceTimings();
            }"""
        )

    def resources(self):
        return self.page.evaluate(
            """() => performance.getEntriesByType('resource').filter(e =>
                window.__btSince === undefined || e.startTime >= window.__btSince
            ).map(e => ({
                name: e.name,
                transferSize: e.transferSize,
                encodedBodySize: e.encodedBodySize,
                decodedBodySize: e.decodedBodySize,
                initiatorType: e.initiatorType,
                startTime: e.startTime
            }))"""
        )

    def navigation(self):
        return self.page.evaluate(
            """() => {
                const e = performance.getEntriesByType('navigation')[0];
                if (!e) return null;
                return {
                    transferSize: e.transferSize,
                    encodedBodySize: e.encodedBodySize,
                    decodedBodySize: e.decodedBodySize
                };
            }"""
        )

    def assert_no_shell_transfer(self, where):
        assert self.shell_alive(), f"{where}: the document reloaded (shell marker is gone)"
        moved = []
        wire = []
        for entry in self.resources():
            if entry["transferSize"] <= 0:
                continue
            path = urlparse(entry["name"]).path
            wire.append(
                {
                    "path": path,
                    "transferSize": entry["transferSize"],
                    "decodedBodySize": entry["decodedBodySize"],
                }
            )
            if shell_path(entry["name"]):
                moved.append(entry)
        MEASUREMENTS[where] = wire
        assert not moved, f"{where}: shell transferred again: {json.dumps(moved)}"
        bulky = [entry for entry in wire if entry["transferSize"] > SHOULD_COMPRESS]
        assert not bulky, (
            f"{where}: a large body transferred during navigation "
            f"(shell or Syeve): {json.dumps(bulky)}"
        )


def test_in_app_navigation_does_not_redownload_the_shell(browser, base):
    page = Page(browser, base)
    try:
        page.goto_app()
        # The other pipeline is a tab on this page. Same component, new identity: the shell stays.
        page.resources_since_clear()
        page.page.locator("a[href='/pipeline/docs']").click()
        page.page.wait_for_selector("text=spell")
        assert page.page.url.endswith("/pipeline/docs")
        page.assert_no_shell_transfer("other pipeline")

        page.resources_since_clear()
        page.page.locator("a[href='/pipeline/app']").click()
        page.page.wait_for_selector(f"a[href='/task/{TASK}']")
        page.assert_no_shell_transfer("back to app")

        page.resources_since_clear()
        page.page.locator(f"a[href='/task/{TASK}']").click()
        page.page.wait_for_selector("text=shell-nav-marker")
        assert f"/task/{TASK}" in page.page.url
        page.assert_no_shell_transfer("open task")

        page.resources_since_clear()
        page.page.locator("a.page[href='/workers']").click()
        page.page.wait_for_selector("text=No workers yet")
        assert page.page.url.endswith("/workers")
        page.assert_no_shell_transfer("workers")
    finally:
        page.close()


def test_back_and_forward_restore_the_page_without_a_shell_reload(browser, base):
    page = Page(browser, base)
    try:
        page.goto_app()
        page.page.locator(f"a[href='/task/{TASK}']").click()
        page.page.wait_for_selector("text=shell-nav-marker")

        page.resources_since_clear()
        page.page.evaluate("() => history.back()")
        page.page.wait_for_url("**/pipeline/app")
        page.page.wait_for_selector(f"a[href='/task/{TASK}']")
        assert page.page.locator("text=shell-nav-marker").count() == 0
        assert "6dfde7" in page.page.locator("th.run").inner_text()
        assert "unit" in page.page.locator("th.task").inner_text()
        page.assert_no_shell_transfer("back")

        page.resources_since_clear()
        page.page.evaluate("() => history.forward()")
        page.page.wait_for_url(f"**/task/{TASK}")
        page.page.wait_for_selector("text=shell-nav-marker")
        assert "unit" in page.page.locator("header").inner_text()
        page.assert_no_shell_transfer("forward")
    finally:
        page.close()


def test_repeat_visit_uses_the_cache_for_the_shell(browser, base):
    page = Page(browser, base)
    try:
        page.goto_app()
        # The mark is an SVG image, and the fonts load once text uses them. Let them finish so the
        # reload is a cache check, not a race with the first fetch.
        page.page.wait_for_function(
            """() => {
                const images = [...document.images];
                return images.length > 0 && images.every(img => img.complete)
                    && document.fonts.status === "loaded";
            }"""
        )
        page.page.reload(wait_until="domcontentloaded")
        page.page.wait_for_selector(f"a[href='/task/{TASK}']")
        nav = page.navigation()
        assert nav is not None
        MEASUREMENTS["repeat_document"] = nav
        assert 0 < nav["transferSize"] < FIRST_BURST, (
            f"repeat document wire size {nav['transferSize']} is outside a small recheck"
        )
        seen = []
        for entry in page.resources():
            path = urlparse(entry["name"]).path
            # wasm, JS, CSS, and the fonts are the shell that must come from cache.
            # Chrome sometimes refetches a small SVG image on reload anyway; that is a few
            # hundred bytes, and the large-asset check below still fails a real miss.
            if not path.endswith(SHELL_SUFFIXES):
                continue
            seen.append(entry)
            assert entry["transferSize"] == 0, (
                f"repeat visit transferred {entry['name']} "
                f"({entry['transferSize']} bytes on the wire, "
                f"decoded {entry['decodedBodySize']})"
            )
        names = [urlparse(entry["name"]).path for entry in seen]
        assert any(name.endswith(".wasm") for name in names), (
            f"reload did not record the wasm at all, so the cache claim is unmeasured: {names}"
        )
        again = [
            entry
            for entry in page.resources()
            if entry["transferSize"] > SHOULD_COMPRESS
        ]
        assert not again, (
            f"repeat visit transferred a large asset with no cache: {json.dumps(again)}"
        )
    finally:
        page.close()


def test_transfer_sizes_stay_within_the_first_burst_rule(browser, base):
    page = Page(browser, base)
    try:
        page.goto_app()
        # The frame is up. Syeve then arrives in the background. The product will do this
        # itself; until then the test requests the stand-in so the budget is measured against
        # a visit that actually downloaded it.
        page.page.evaluate(
            "async (path) => { await (await fetch(path)).arrayBuffer() }",
            SYEVE_PATH,
        )
        nav = page.navigation()
        assert nav is not None, "no navigation timing for the first document"
        assert nav["transferSize"] > 0, "first document was not actually transferred"
        assert nav["transferSize"] < FIRST_BURST, (
            f"first document wire size {nav['transferSize']} exceeds the "
            f"{FIRST_BURST}-byte first burst (headers included). "
            f"This is the document, not the visit: Syeve and the wasm sit outside it."
        )
        by_url = {}
        for response in page.responses:
            url = response.url
            if urlparse(url).netloc != urlparse(page.base).netloc:
                continue
            headers = {k.lower(): v for k, v in response.headers.items()}
            by_url.setdefault(url, []).append(headers)

        transferred = {}
        for entry in page.resources():
            path = urlparse(entry["name"]).path
            headers = by_url.get(entry["name"], [{}])[-1]
            record = {
                "path": path,
                "transferSize": entry["transferSize"],
                "encodedBodySize": entry["encodedBodySize"],
                "decodedBodySize": entry["decodedBodySize"],
                "content-encoding": headers.get("content-encoding", ""),
                "cache-control": headers.get("cache-control", ""),
            }
            transferred.setdefault(path, []).append(record)
            if code_path(entry["name"]) and entry["decodedBodySize"] > SHOULD_COMPRESS:
                assert headers.get("content-encoding") == "gzip", (
                    f"{path} is {entry['decodedBodySize']} bytes decoded and not gzipped"
                )
                assert entry["encodedBodySize"] < entry["decodedBodySize"] * 0.9, (
                    f"{path} barely compressed: encoded {entry['encodedBodySize']} "
                    f"decoded {entry['decodedBodySize']}"
                )
            if path.endswith(SHELL_SUFFIXES) or path.startswith("/assets/"):
                assert "immutable" in headers.get("cache-control", ""), (
                    f"{path} is shell and not cached forever: {headers.get('cache-control')}"
                )
            if path == "/" or path.endswith("/pipeline/app"):
                assert headers.get("cache-control") == "no-cache", headers

        doubles = {
            path: rows
            for path, rows in transferred.items()
            if code_path("http://x" + path)
            and sum(row["transferSize"] for row in rows) > SHOULD_COMPRESS
            and len(rows) > 1
        }
        assert not doubles, f"large assets fetched more than once: {json.dumps(doubles)}"

        wasm = [
            row
            for path, rows in transferred.items()
            if path.endswith(".wasm")
            for row in rows
        ]
        assert wasm, f"wasm was not in the resource timing: {list(transferred)}"
        syeve = transferred.get(SYEVE_PATH, [])
        assert len(syeve) == 1, f"Syeve fetched {len(syeve)} times on the first visit: {syeve}"
        assert syeve[0]["transferSize"] > FIRST_BURST, (
            f"Syeve's transfer ({syeve[0]['transferSize']} bytes) fit in the burst, "
            f"so the document check is no longer proven against it"
        )
        assert "immutable" in syeve[0]["cache-control"], syeve[0]
        visit = nav["transferSize"] + sum(
            row["transferSize"] for rows in transferred.values() for row in rows
        )
        # The visit is over the burst because it includes Syeve and the wasm. The assertion
        # above is only the document. Requiring the visit to fit would drop one of them.
        assert visit > FIRST_BURST, (
            f"first visit transfer {visit} fit in the burst; the document must fit, "
            f"and the visit (wasm + Syeve) must stay allowed to exceed it"
        )
        MEASUREMENTS["first_document"] = nav
        MEASUREMENTS["syeve"] = syeve[0]
        MEASUREMENTS["first_visit_transfer"] = visit
        MEASUREMENTS["resources"] = {
            path: rows for path, rows in sorted(transferred.items())
        }
        decoded = wasm[0]["decodedBodySize"]
        encoded = wasm[0]["encodedBodySize"]
        print(
            f"document transfer {nav['transferSize']} (burst {FIRST_BURST}); "
            f"wasm decoded {decoded} encoded {encoded}; "
            f"syeve transfer {syeve[0]['transferSize']}; visit {visit}"
        )
        assert decoded > FIRST_BURST or encoded > 0
    finally:
        page.close()


def test_webkit_starts_the_app_over_tls(webkit, tls_base, iphone):
    """iPhone Safari once got the page from the boss but never started the app (#15, when the
    boss spoke HTTP/2). WebKit as an iPhone, over TLS as the boss speaks it: the app starts on a
    first visit, and again on a second one in a new tab whose connections resume the session."""
    context = webkit.new_context(ignore_https_errors=True, **iphone)
    try:
        for visit in ("first", "second"):
            del RESUMED[:]
            page = context.new_page()
            page.goto(tls_base + "/pipeline/app", wait_until="domcontentloaded")
            page.wait_for_selector(f"a[href='/task/{TASK}']", timeout=30_000)
            MEASUREMENTS[f"webkit_tls_{visit}_visit_resumed"] = RESUMED[:]
            page.close()
        assert any(RESUMED), f"the second visit resumed no TLS session: {RESUMED}"
    finally:
        context.close()


# Every animation frame until the app starts, as the page would paint it.
RECORD_FRAMES = """
window.__btFrames = [];
const sample = () => {
    if (document.body?.hasAttribute("data-started")) return;
    if (document.body) window.__btFrames.push(document.body.innerText);
    requestAnimationFrame(sample);
};
requestAnimationFrame(sample);
"""


def test_nothing_is_painted_before_the_app(browsers, base):
    """The first visible frame is the page itself: no wordmark-only shell before the app."""
    for name, (browser, device) in browsers.items():
        context = browser.new_context(**device)
        try:
            context.add_init_script(RECORD_FRAMES)
            page = context.new_page()
            page.goto(base + "/pipeline/app", wait_until="domcontentloaded")
            page.wait_for_selector(f"a[href='/task/{TASK}']", timeout=30_000)
            frames = page.evaluate("window.__btFrames")
            assert frames, f"{name}: no frame was sampled before the app started"
            painted = [text for text in frames if text.strip()]
            assert not painted, f"{name}: painted before the app started: {painted[:3]}"
        finally:
            context.close()


def test_a_browser_that_cannot_start_the_app_says_why(browsers, base):
    """Rather than a blank page: the reason, on the page and to the boss."""
    BREAK_WASM.set()
    try:
        for name, (browser, device) in browsers.items():
            del STARTUP_ERRORS[:]
            context = browser.new_context(**device)
            try:
                page = context.new_page()
                page.goto(base + "/pipeline/app", wait_until="domcontentloaded")
                page.wait_for_selector("body[data-failed]", timeout=30_000)
                shown = page.inner_text("body")
                assert shown.startswith("bettertest could not start: "), f"{name}: {shown!r}"
                page.wait_for_timeout(500)
                assert STARTUP_ERRORS, f"{name}: the boss heard nothing"
                assert STARTUP_ERRORS[0] in shown, f"{name}: {STARTUP_ERRORS[0]!r} vs {shown!r}"
                MEASUREMENTS[f"{name}_startup_error"] = STARTUP_ERRORS[0]
            finally:
                context.close()
    finally:
        BREAK_WASM.clear()


TESTS = [
    test_in_app_navigation_does_not_redownload_the_shell,
    test_back_and_forward_restore_the_page_without_a_shell_reload,
    test_repeat_visit_uses_the_cache_for_the_shell,
    test_transfer_sizes_stay_within_the_first_burst_rule,
]


def main():
    # The browser side runs in Playwright's image, which has no trunk.
    if dist_stale():
        raise SystemExit("frontend/dist is missing or stale: run python3 test/shell.py build first")
    server, base = start_server()
    tls_server, tls_base = start_server(tls=True)
    from playwright.sync_api import sync_playwright

    failed = 0

    def run(test, *args):
        nonlocal failed
        try:
            test(*args)
            print(f"PASS {test.__name__}")
        except Exception:
            failed += 1
            print(f"FAIL {test.__name__}")
            traceback.print_exc()

    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                args=["--no-sandbox", "--disable-dev-shm-usage"],
            )
            webkit = playwright.webkit.launch()
            try:
                for test in TESTS:
                    run(test, browser, base)
                iphone = playwright.devices["iPhone 15"]
                run(test_webkit_starts_the_app_over_tls, webkit, tls_base, iphone)
                both = {"chromium": (browser, {}), "webkit": (webkit, iphone)}
                run(test_nothing_is_painted_before_the_app, both, base)
                run(test_a_browser_that_cannot_start_the_app_says_why, both, base)
            finally:
                browser.close()
                webkit.close()
    finally:
        server.shutdown()
        tls_server.shutdown()
    if MEASUREMENTS:
        print(
            "content-bytes",
            json.dumps(
                {
                    "pipeline_app_snapshot": len(PIPELINES["app"]),
                    "pipeline_docs_snapshot": len(PIPELINES["docs"]),
                    "task_snapshot": len(TASKS[TASK]),
                    "workers_snapshot": len(WORKERS),
                }
            ),
        )
        print("MEASUREMENTS")
        print(json.dumps(MEASUREMENTS, indent=2))
    if failed:
        raise SystemExit(f"{failed} shell test(s) failed")


if __name__ == "__main__":
    if sys.argv[1:] == ["build"]:
        build_dist()
    else:
        main()
