"""Loopback-only HTTP server of the screen (/) and the Project API."""

from __future__ import annotations

import json
import mimetypes
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

from sheltersim.storage.projects import DEFAULT_MAX_BYTES, ProjectStore, StorageError
from sheltersim.web.projects import ProjectAPI, answers

# A refusal of the core's answers: (HTTP status, error, code, the body to answer with).
REFUSED = answers().Refused

_STATIC = Path(__file__).with_name("static")


# The types of the files the server sends. mimetypes reads them from the system (the
# registry on Windows, which calls JavaScript application/javascript), so the files
# of the app are fixed here and every platform sends the same.
_CONTENT_TYPES = {
    ".css": "text/css",
    ".html": "text/html",
    ".jpeg": "image/jpeg",
    ".jpg": "image/jpeg",
    ".js": "text/javascript",
    ".json": "application/json",
    ".mjs": "text/javascript",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".ttf": "font/ttf",
    ".wasm": "application/wasm",
    ".woff2": "font/woff2",
}


def _content_type(name: str) -> str:
    suffix = Path(name).suffix.lower()
    return _CONTENT_TYPES.get(suffix) or mimetypes.guess_type(name)[0] or "application/octet-stream"


def llm_status(settings: Any) -> dict[str, Any]:
    """`GET /api/v1/llm` (#581): whether the server's AI (LLM) features are ON, from its LLM
    settings: ON with an API key in record mode, or in replay mode (the recorded answers). The
    screen shows it at its top and on the home; the browser version asks its relay instead
    (the home's passphrase). The key itself is never answered."""
    from sheltersim.llm.config import LLMMode

    mode = settings.effective_mode
    if mode is LLMMode.OFFLINE:
        reason = "サーバーがAIを使わない設定（offline）で起動しています。"
    elif mode is LLMMode.RECORD and not settings.api_key:
        reason = "サーバーにAPIキーが設定されていません。"
    else:
        reason = None
    return {"available": reason is None, "mode": mode.value, "reason": reason}


def serve_inspector(
    runs_directory: str | Path,
    *,
    host: str = "127.0.0.1",
    port: int = 8765,
    max_parallel_runs: int = 2,
    project_storage_bytes: int = DEFAULT_MAX_BYTES,
) -> None:
    """Serve the screen (/) and the Project API until interrupted.

    Projects are saved under runs_directory/.projects. Up to max_parallel_runs Runs
    compute or fork at once, each in its own process. The projects directory holds at
    most project_storage_bytes of saved data; the computing and indexing processes get
    the same limit from the store.
    """
    project_store = ProjectStore(
        Path(runs_directory) / ".projects", max_bytes=project_storage_bytes
    )
    projects = ProjectAPI(project_store, worker="process", max_jobs=max_parallel_runs)
    handler = _handler_for(projects)
    server = HTTPServer((host, port), handler)
    print(f"避難所IF！: http://{host}:{server.server_port}/")
    try:
        server.serve_forever()
    finally:
        server.server_close()
        projects.close()


# The screen is at / (#376); /index.html goes to it with the query kept. The earlier paths
# (/demo-v2, /demo, /admin) are not redirected: no one expects the older versions (#434).
_REDIRECTS = frozenset({"/index.html"})

# The files of the screen, by the path they are served at.
_STATIC_FILES = {
    "/": "demo-v2.html",
    "/demo-v2.css": "demo-v2.css",
    "/demo-v2-logo.jpg": "demo-v2-logo.jpg",
    "/demo-v2-header-logo.png": "demo-v2-header-logo.png",
    "/demo-v2.js": "demo-v2.js",
    "/demo-v2-map.js": "demo-v2-map.js",
    "/demo-v2-scene.js": "demo-v2-scene.js",
    "/demo-v2-clock.js": "demo-v2-clock.js",
    "/demo-v2-replay.js": "demo-v2-replay.js",
    "/demo-v2-sprites.js": "demo-v2-sprites.js",
    "/demo-v2-state.js": "demo-v2-state.js",
    "/demo-v2-text.js": "demo-v2-text.js",
    "/demo-v2-api.js": "demo-v2-api.js",
    "/demo-v2-pages.js": "demo-v2-pages.js",
    "/demo-v2-draw.js": "demo-v2-draw.js",
    "/demo-v2-home.js": "demo-v2-home.js",
    "/demo-v2-scenario.js": "demo-v2-scenario.js",
    "/demo-v2-simulation.js": "demo-v2-simulation.js",
    "/demo-v2-people.js": "demo-v2-people.js",
    "/demo-v2-comparison.js": "demo-v2-comparison.js",
    "/demo-v2-report.js": "demo-v2-report.js",
    "/demo-v2-print.js": "demo-v2-print.js",
    "/demo-v2-detail.js": "demo-v2-detail.js",
}


def _handler_for(projects: ProjectAPI | None = None) -> type[BaseHTTPRequestHandler]:
    class InspectorHandler(BaseHTTPRequestHandler):
        server_version = "ShelterSimInspector/1"

        def do_GET(self) -> None:  # noqa: N802 - stdlib HTTP hook name
            parsed = urlsplit(self.path)
            try:
                if parsed.path.startswith("/api/v1/"):
                    if parsed.path == "/api/v1/llm":
                        if parsed.query:
                            raise StorageError("the LLM status does not accept query parameters")
                        from sheltersim.llm.config import LLMSettings

                        payload = llm_status(LLMSettings.from_env())
                    elif parsed.path == "/api/v1/usage/runtime":
                        if parsed.query:
                            raise StorageError("runtime usage does not accept query parameters")
                        if projects is None:
                            raise StorageError("usage storage is unavailable")
                        from sheltersim.llm.accounting import runtime_usage_summary

                        payload = runtime_usage_summary(projects.store)
                    elif parsed.path == "/api/v1/projects" or parsed.path.startswith(
                        "/api/v1/projects/"
                    ):
                        if projects is None:
                            raise StorageError("project storage is unavailable")
                        parts = [unquote(p) for p in parsed.path.split("/") if p][3:]
                        self._raw_response(200, projects.answer_get(parts, parsed.query))
                        return
                    else:
                        self._json_response(400, {"error": "unknown API route"})
                        return
                    self._json_response(200, payload)
                elif parsed.path in _REDIRECTS:
                    target = "/"
                    if parsed.query:
                        target = f"{target}?{parsed.query}"
                    self.send_response(302)
                    self.send_header("Location", target)
                    self.send_header("Content-Length", "0")
                    self.send_header("Cache-Control", "no-store")
                    self.end_headers()
                else:
                    self._static_response(parsed.path)
            except REFUSED as exc:
                self._raw_response(exc.args[0], exc.args[3])
            except StorageError as exc:
                self._json_response(409, {"error": str(exc), "code": exc.code})
            except ValueError:
                self._json_response(
                    400, {"error": "invalid query or saved contract", "code": "invalid_input"}
                )
            except Exception:
                self._json_response(500, {"error": "inspector request failed"})

        def do_POST(self) -> None:  # noqa: N802 - stdlib HTTP hook name
            parsed = urlsplit(getattr(self, "path", ""))
            parts = [unquote(part) for part in parsed.path.split("/") if part]
            origin = self.headers.get("Origin")
            if (origin and urlsplit(origin).netloc != self.headers.get("Host")) or self.headers.get(
                "Sec-Fetch-Site"
            ) == "cross-site":
                self._json_response(403, {"error": "cross-origin mutations are forbidden"})
                return
            if self.headers.get("Content-Type", "").split(";")[0] != "application/json":
                self._json_response(415, {"error": "mutations require application/json"})
                return
            if parsed.query or parts[:3] != ["api", "v1", "projects"]:
                self._json_response(405, {"error": "inspector API is read-only"})
                return
            try:
                if projects is None:
                    raise StorageError("project storage is unavailable")
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 2_097_152:
                    raise StorageError("project request exceeds the body limit")
                body = self.rfile.read(length)
                document = json.loads(body.decode("utf-8"))
                if not isinstance(document, dict):
                    raise StorageError("project request must be an object")
                self._raw_response(200, projects.answer_post(parts[3:], body))
            except REFUSED as exc:
                self._raw_response(exc.args[0], exc.args[3])
            except StorageError as exc:
                self._json_response(409, {"error": str(exc), "code": exc.code})
            except (ValueError, UnicodeDecodeError):
                self._json_response(400, {"error": "invalid project request"})
            except Exception:
                self._json_response(500, {"error": "project operation failed"})

        def do_PUT(self) -> None:  # noqa: N802 - stdlib HTTP hook name
            self._json_response(405, {"error": "inspector API is read-only"})

        def do_DELETE(self) -> None:  # noqa: N802 - stdlib HTTP hook name
            self._json_response(405, {"error": "inspector API is read-only"})

        def _json_response(self, status: int, document: Any) -> None:
            self._raw_response(
                status,
                json.dumps(
                    document,
                    ensure_ascii=False,
                    sort_keys=True,
                    separators=(",", ":"),
                    allow_nan=False,
                ).encode("utf-8"),
            )

        def _raw_response(self, status: int, payload: bytes) -> None:
            """An answer whose JSON bytes are written already (the core's)."""
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Content-Security-Policy", "default-src 'self'; style-src 'self'")
            self.end_headers()
            self.wfile.write(payload)

        def _static_response(self, request_path: str) -> None:
            filename = _STATIC_FILES.get(request_path)
            if filename is None:
                self._json_response(404, {"error": "page not found"})
                return
            payload = (_STATIC / filename).read_bytes()
            self.send_response(200)
            content_type = _content_type(filename)
            self.send_header("Content-Type", f"{content_type}; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header(
                "Content-Security-Policy",
                "default-src 'self'; style-src 'self'; img-src 'self' data: blob:",
            )
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: Any) -> None:
            # Keep routine local queries quiet; malformed/failed requests still get HTTP errors.
            del format, args

    return InspectorHandler
