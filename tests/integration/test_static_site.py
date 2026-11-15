"""The browser version's static site (#266): scripts/build_static_site.py assembles it and
scripts/serve_static_site.py serves it as a static host would.

The site is built from a stand-in WASM package and a small bundle (only its header and manifest
are read; the browser checks the rest): the screen's files are copied unchanged, demo-v2.html
gains the one line that loads the browser's API, the bundle is split into parts no larger than
asked that join back to it, and `/` is the same page, which opens on its home
(#371). The server answers `/demo-v2` with
demo-v2.html and modules and WASM with their types over HTTP. The browser's half is checked by
native/wasm/tests/node_browser_api.mjs and docs/development.md's browser checks.

The site also holds the licenses of what its WASM holds (#290): the notices of the crates it links,
made from `cargo tree` and `cargo metadata` (checked on stand-in packages, and on this repository's
Cargo.lock where cargo and the crates are at hand), and the Rust standard library's.

The site is published as Cloudflare Workers' static assets with deploy/workers/wrangler.jsonc
(#289): its settings must answer as the local server does, which wrangler's own dry run cannot tell.
"""

from __future__ import annotations

import hashlib
import json
import re
import runpy
import shutil
import struct
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).parents[2]
STATIC = ROOT / "src" / "sheltersim" / "web" / "static"


@pytest.fixture(scope="module")
def builder() -> dict[str, Any]:
    return runpy.run_path(str(ROOT / "scripts" / "build_static_site.py"))


@pytest.fixture(scope="module")
def server_module() -> dict[str, Any]:
    return runpy.run_path(str(ROOT / "scripts" / "serve_static_site.py"))


def _bundle(path: Path, size: int) -> bytes:
    """A file with a project bundle's header and manifest (Project `sample`, one Run), padded."""
    manifest = json.dumps(
        {
            "format": "sheltersim-project-bundle",
            "version": 2,
            "project_id": "sample",
            "source": None,
            "entries": [
                {"path": "project.json", "length": 0, "sha256": "0" * 64},
                {"path": "runs/run-x/run.json", "length": 0, "sha256": "0" * 64},
                {"path": "runs/run-x/branches/main/branch.json", "length": 0, "sha256": "0" * 64},
            ],
        }
    ).encode()
    head = b"\x89SHELTERSIM-PRJ\n" + struct.pack(">II", 2, len(manifest))
    data = head + hashlib.sha256(manifest).digest() + manifest
    data += bytes(index % 251 for index in range(size - len(data)))
    path.write_bytes(data)
    return data


# Stand-ins for the notices third_party_notices() writes from cargo and the toolchain.
NOTICES = {
    "THIRD-PARTY-NOTICES.txt": b"the crates' notices\n",
    "THIRD-PARTY-NOTICES-rust-library.html": b"<html>the Rust library's notices</html>\n",
}


@pytest.fixture
def site(builder: dict[str, Any], tmp_path: Path) -> tuple[Path, bytes, dict[str, Any]]:
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "sheltersim_wasm.js").write_text("export default async function init() {}\n")
    (pkg / "sheltersim_wasm_bg.wasm").write_bytes(b"\0asm\1\0\0\0")
    bundle = _bundle(tmp_path / "sample.shelterproj", 2 * 2**20 + 12345)
    output = tmp_path / "site"
    summary = builder["build"](
        output, pkg=pkg, bundle=tmp_path / "sample.shelterproj", part_mib=1, notices=NOTICES
    )
    return output, bundle, summary


def test_the_site_holds_the_screen_the_browser_api_and_the_sample_in_parts(
    site: tuple[Path, bytes, dict[str, Any]], builder: dict[str, Any]
) -> None:
    output, bundle, summary = site
    RELATIVE_IMPORT, JS = builder["RELATIVE_IMPORT"], builder["JS"]
    for name in ("demo-v2.css", "demo-v2.js", "demo-v2-map.js", "demo-v2-logo.jpg"):
        assert (output / name).read_bytes() == (STATIC / name).read_bytes()
    server_html = (STATIC / "demo-v2.html").read_text(encoding="utf-8")
    browser_html = (output / "demo-v2.html").read_text(encoding="utf-8")
    added = [line for line in browser_html.splitlines() if line not in server_html.splitlines()]
    assert added == [
        '    <script src="/browser/api_fetch.js" data-worker="/browser/api_worker.mjs"'
        ' data-examples="/sample/examples.json" data-example-project="sample"'
        # Built without the shelter index, the site cannot make Scenarios and says so (#372).
        ' data-scenarios="unavailable"></script>',
    ]
    # The .txt files begin with UTF-8's byte order mark: a host answers them without a charset
    # (Workers' static assets do), and the browser then reads the license's Japanese in another
    # encoding (#410).
    bom = b"\xef\xbb\xbf"
    license_text = (STATIC.parents[3] / "LICENSE").read_bytes()
    assert "避難所IF！".encode() in license_text
    assert (output / "LICENSE.txt").read_bytes() == bom + license_text
    for name, data in NOTICES.items():
        assert (output / name).read_bytes() == (bom + data if name.endswith(".txt") else data)
    assert browser_html.index("/browser/api_fetch.js") < browser_html.index("/demo-v2-scene.js")
    for name in ("api_fetch.js", "api_worker.mjs", "browser_api.mjs", "compute.mjs"):
        assert (output / "browser" / name).is_file()
    # Every module the site's modules import beside them is on the site (a missing one stops the
    # Worker from starting).
    browser = output / "browser"
    for module in browser.glob("*.*js"):
        for name in RELATIVE_IMPORT.findall(module.read_text(encoding="utf-8")):
            assert (browser / name).read_bytes() == (JS / name).read_bytes(), (module.name, name)
    assert (output / "browser" / "pkg" / "sheltersim_wasm_bg.wasm").is_file()

    manifest = json.loads((output / "sample" / "sample.json").read_text(encoding="utf-8"))
    assert manifest["format"] == "sheltersim-site-sample" and manifest["version"] == 1
    assert manifest["project_id"] == "sample" and manifest["bytes"] == len(bundle)
    assert manifest["sha256"] == hashlib.sha256(bundle).hexdigest()
    parts = [(output / "sample" / part["path"]).read_bytes() for part in manifest["parts"]]
    assert len(parts) == 3 and all(len(part) <= 2**20 for part in parts)
    assert b"".join(parts) == bundle
    assert [hashlib.sha256(part).hexdigest() for part in parts] == [
        part["sha256"] for part in manifest["parts"]
    ]
    # The list of examples the home offers to fetch (#411): the sample's manifest and what the
    # bundle says of itself (this one holds no Scenario).
    examples = json.loads((output / "sample" / "examples.json").read_text(encoding="utf-8"))
    assert examples == {
        "format": "sheltersim-site-examples",
        "version": 1,
        "examples": [
            {
                "manifest": "sample.json",
                "project_id": "sample",
                "bytes": len(bundle),
                "sha256": manifest["sha256"],
                "runs": 1,
                "branches": 1,
                "municipality": None,
                "disaster": None,
                "people": None,
                "duration_minutes": None,
            }
        ],
    }
    assert summary["open"] == "/?project=sample&run=run-x&branch=main#simulation"
    assert "compute" not in summary
    # `/` is the screen itself, which opens on its home: the examples (fetched when chosen,
    # api_fetch.js), one's own Scenarios, and the licenses the site holds.
    assert (output / "index.html").read_text(encoding="utf-8") == browser_html
    for name in ("LICENSE.txt", *NOTICES):
        assert f'href="/{name}"' in browser_html and (output / name).is_file()
    worker_js = (STATIC.parents[3] / "native" / "wasm" / "js" / "api_worker.mjs").read_text("utf-8")
    assert 'root.getDirectoryHandle("projects", { create: true })' in worker_js
    assert summary["largest"]["bytes"] <= 2**20


def test_the_list_of_examples_says_what_a_bundle_holds_before_it_is_fetched(
    builder: dict[str, Any], tmp_path: Path
) -> None:
    """describe_bundle (#411): the Runs, the branches, and of the first Scenario the municipality,
    the disaster, the number of people and the minutes, read from the bundle's entries."""
    files = [
        ("project.json", b"{}"),
        ("scenarios/s1/v1.json", b'{"approved": true}'),
        (
            "scenarios/s1/v1.scenario.json",
            json.dumps(
                {
                    "duration_minutes": 10080,
                    "profiles": [{}, {}, {}],
                    "provenance": {
                        "natural_language_input": {
                            "municipality": "墨田区",
                            "disaster": "earthquake",
                        }
                    },
                }
            ).encode(),
        ),
        ("runs/run-x/run.json", b"{}"),
        ("runs/run-x/branches/main/branch.json", b"{}"),
        ("runs/run-x/branches/b-1/branch.json", b"{}"),
    ]
    manifest = json.dumps(
        {
            "format": "sheltersim-project-bundle",
            "version": 1,
            "project_id": "sample",
            "source": None,
            "entries": [
                {"path": path, "length": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                for path, data in files
            ],
        }
    ).encode()
    head = b"\x89SHELTERSIM-PRJ\n" + struct.pack(">II", 1, len(manifest))
    bundle = tmp_path / "described.shelterproj"
    bundle.write_bytes(
        head + hashlib.sha256(manifest).digest() + manifest + b"".join(d for _, d in files)
    )
    assert builder["describe_bundle"](bundle) == {
        "runs": 1,
        "branches": 2,
        "municipality": "墨田区",
        "disaster": "earthquake",
        "people": 3,
        "duration_minutes": 10080,
    }


def test_the_site_holds_the_relay_s_owner_page_at_one_path_not_linked_from_the_screen(
    site: tuple[Path, bytes, dict[str, Any]], builder: dict[str, Any], server_module: dict[str, Any]
) -> None:
    """The owner's page of the LLM relay (#433): written at ADMIN_PAGE (the one place naming its
    path, which the summary repeats for the deploy workflow's check), not indexed, holding no key,
    calling only the relay's admin routes, and not linked from the screen."""
    output, _, summary = site
    page_path = builder["ADMIN_PAGE"]
    assert summary["admin_page"] == f"/{page_path}"
    assert not page_path.startswith("llm/")
    written = output / f"{page_path}.html"
    assert written.read_bytes() == builder["ADMIN_SOURCE"].read_bytes()
    page = written.read_text(encoding="utf-8")
    assert '<meta name="robots" content="noindex, nofollow" />' in page
    assert re.findall(r"fetch\(`([^`$]*)", page) == ["/llm/v1/admin/"]
    assert "localStorage" not in page and "sessionStorage" not in page
    assert not re.search(r"(sk-ant-|LLM_ADMIN_KEY\s*=|Bearer [A-Za-z0-9_-]{16,})", page)
    for screen in ("demo-v2.html", "index.html"):
        assert page_path not in (output / screen).read_text(encoding="utf-8")
    assert server_module["resolve"](output, f"/{page_path}") == written


def test_the_site_is_built_only_outside_the_repository_into_an_empty_directory(
    builder: dict[str, Any], site: tuple[Path, bytes, dict[str, Any]], tmp_path: Path
) -> None:
    output, _, _ = site
    arguments: dict[str, Any] = {
        "pkg": output / "browser" / "pkg",
        "bundle": tmp_path / "sample.shelterproj",
        "notices": NOTICES,
    }
    with pytest.raises(ValueError, match="outside the repository"):
        builder["build"](ROOT / "site-out", **arguments)
    with pytest.raises(ValueError, match="not empty"):
        builder["build"](output, **arguments)
    with pytest.raises(ValueError, match="no Run"):
        builder["build"](tmp_path / "other", run_id="nope", **arguments)
    with pytest.raises(ValueError, match="--part-mib"):
        builder["build"](tmp_path / "third", part_mib=21, **arguments)
    with pytest.raises(ValueError, match="notices"):
        builder["build"](
            tmp_path / "fourth", **(arguments | {"notices": {"THIRD-PARTY-NOTICES.txt": b""}})
        )
    assert not (ROOT / "site-out").exists()


def test_the_site_holds_the_shelter_index_and_rows_the_browser_s_research_reads(
    builder: dict[str, Any], site: tuple[Path, bytes, dict[str, Any]], tmp_path: Path
) -> None:
    """With a copy of the national shelter CSV, the site holds its `gsi_shelter_index` (#300)
    and each municipality code's `gsi_shelter_rows` (#332); without one it holds neither."""
    from support.core_research import EXTRA_CSV, shelter_index, shelter_rows

    output, _, summary = site
    assert summary["shelter_index"] is None
    assert not (output / "browser" / "gsi-shelter-index.json").exists()
    assert not (output / "browser" / "gsi-shelters").exists()
    csv_path = tmp_path / "mergeFromCity_1.csv"
    csv_path.write_bytes(b"\xef\xbb\xbf" + EXTRA_CSV.encode("utf-8"))
    files = builder["shelter_files"](csv_path=csv_path)
    made = files["index"]
    expected = shelter_index(EXTRA_CSV)
    assert made["sha256"] == hashlib.sha256(csv_path.read_bytes()).hexdigest()
    assert {key: value for key, value in made.items() if key not in ("sha256", "retrieved_at")} == {
        key: value for key, value in expected.items() if key not in ("sha256", "retrieved_at")
    }
    assert {code: rows["rows"] for code, rows in files["rows"].items()} == {
        code: rows["rows"] for code, rows in shelter_rows(EXTRA_CSV).items()
    }
    assert {rows["sha256"] for rows in files["rows"].values()} == {made["sha256"]}
    arguments = {
        "pkg": output / "browser" / "pkg",
        "bundle": tmp_path / "sample.shelterproj",
        "notices": NOTICES,
    }
    with_index = builder["build"](tmp_path / "with-index", shelters=files, **arguments)
    page = (tmp_path / "with-index" / "demo-v2.html").read_text(encoding="utf-8")
    assert 'data-example-project="sample"></script>' in page
    assert "data-scenarios" not in page
    browser = tmp_path / "with-index" / "browser"
    assert json.loads((browser / "gsi-shelter-index.json").read_text(encoding="utf-8")) == made
    written = sorted(path.name for path in (browser / "gsi-shelters").iterdir())
    assert written == [f"{code}.json" for code in made["codes"]]
    assert (
        json.loads((browser / "gsi-shelters" / "13107.json").read_text(encoding="utf-8"))
        == (files["rows"]["13107"])
    )
    assert with_index["shelter_index"]["municipality_codes"] == len(made["codes"])
    assert with_index["shelter_index"]["rows"] == sum(
        len(rows["rows"]) for rows in files["rows"].values()
    )
    assert (browser / "research.mjs").is_file()
    assert builder["shelter_files"]() is None
    pinned = builder["shelter_files"](csv_path=csv_path, retrieved_at="2026-11-01T12:00:00+09:00")
    assert pinned["index"]["retrieved_at"] == "2026-11-01T03:00:00Z"
    csv_path.write_bytes(b"not,a,shelter,csv\n")
    with pytest.raises(ValueError, match="schema"):
        builder["shelter_files"](csv_path=csv_path)


def _package(root: Path, name: str, version: str, files: dict[str, str], **fields: Any) -> dict:
    """A package of `cargo metadata` whose directory holds `files`."""
    directory = root / f"{name}-{version}"
    directory.mkdir()
    (directory / "Cargo.toml").write_text("")
    for file, text in files.items():
        (directory / file).write_bytes(text.encode())
    return {
        "id": f"registry#{name}@{version}",
        "name": name,
        "version": version,
        "manifest_path": str(directory / "Cargo.toml"),
        "license": None,
        "license_file": None,
        "repository": None,
    } | fields


def test_the_notices_hold_each_crate_s_license_files_and_each_text_once(
    builder: dict[str, Any], tmp_path: Path
) -> None:
    """#290: each crate the WASM links but the workspace's own is listed with its license, its
    repository and its license and notice files, whose texts follow once each. A crate whose
    package holds no such file is refused."""
    apache = "Apache License\nVersion 2.0\n"
    packages = [
        _package(
            tmp_path,
            "alpha",
            "1.0.0",
            {"LICENSE-APACHE": apache, "LICENSE-MIT": "Copyright (c) Alpha\r\n", "README.md": ""},
            license="MIT OR Apache-2.0",
            repository="https://example.org/alpha",
        ),
        _package(
            tmp_path,
            "beta",
            "0.2.0",
            {"LICENSE-APACHE": apache, "LEGAL.txt": "Beta's own terms\n"},
            license_file="LEGAL.txt",
        ),
        _package(tmp_path, "ours", "0.1.0", {}),
        _package(tmp_path, "unused", "9.9.9", {}),
    ]
    metadata = {"packages": packages, "workspace_members": ["registry#ours@0.1.0"]}
    text = builder["crate_notices"](
        metadata, [("alpha", "1.0.0"), ("beta", "0.2.0"), ("ours", "0.1.0")]
    )
    assert (
        "alpha 1.0.0\n"
        "  License: MIT OR Apache-2.0\n"
        "  Repository: https://example.org/alpha\n"
        "  Files: LICENSE-APACHE [1], LICENSE-MIT [2]\n"
    ) in text
    assert (
        "beta 0.2.0\n"
        "  License: see its files\n"
        "  Repository: -\n"
        "  Files: LICENSE-APACHE [1], LEGAL.txt [3]\n"
    ) in text
    assert text.endswith(
        "\nTexts\n=====\n\n[1]\n\nApache License\nVersion 2.0\n"
        "\n[2]\n\nCopyright (c) Alpha\n\n[3]\n\nBeta's own terms\n"
    )
    assert "ours" not in text and "unused" not in text
    with pytest.raises(ValueError, match="no license file in the packages of unused 9.9.9"):
        builder["crate_notices"](metadata, [("alpha", "1.0.0"), ("unused", "9.9.9")])
    with pytest.raises(ValueError, match="0 packages gamma 1.0.0"):
        builder["crate_notices"](metadata, [("gamma", "1.0.0")])


def test_the_notices_of_the_repository_s_wasm_name_every_crate_it_links(
    builder: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    """#290: the notices of this repository's Cargo.lock: the WASM's crates as `cargo tree` lists
    them, each with its files. Skipped without cargo, or where the crates are not downloaded
    (cargo is kept offline here)."""
    if shutil.which("cargo") is None or shutil.which("rustc") is None:
        pytest.skip("no cargo")
    monkeypatch.setenv("CARGO_NET_OFFLINE", "true")
    try:
        crates = builder["wasm_crates"]()
        notices = builder["third_party_notices"]()
    except ValueError as error:
        if "offline" in str(error) or "download" in str(error):
            pytest.skip(f"the crates are not downloaded: {error}")
        raise
    names = {name for name, _ in crates}
    assert {"sheltersim_wasm", "sheltersim_core", "wasm-bindgen", "serde_json"} <= names
    # Proc-macro crates run in the compiler, and the core's `parallel` feature is off in the WASM.
    assert not names & {"wasm-bindgen-macro", "syn", "rayon"}
    text = notices["THIRD-PARTY-NOTICES.txt"].decode()
    listed = re.findall(r"^(\S+) (\S+)\n  License: ", text, re.M)
    assert listed == [
        crate for crate in crates if crate[0] not in ("sheltersim_wasm", "sheltersim_core")
    ]
    assert b"Rust Standard Library" in notices["THIRD-PARTY-NOTICES-rust-library.html"]


def test_the_published_config_answers_the_site_s_paths_as_the_local_server_does(
    site: tuple[Path, bytes, dict[str, Any]], builder: dict[str, Any]
) -> None:
    """deploy/workers/wrangler.jsonc (#289): a Worker whose files come from `--assets` (the site
    is built outside the repository), serving `/demo-v2` from demo-v2.html and a missing path
    with the site's 404.html; its script (the LLM relay, #419) answers `/llm/*` only, so every
    other path is the files' as before. No file is larger than Workers' 25 MiB, and the site
    holds none of the files that would change how Workers answer."""
    output, _, _ = site
    config = json.loads((ROOT / "deploy" / "workers" / "wrangler.jsonc").read_text("utf-8"))
    assert set(config) == {
        "name",
        "main",
        "compatibility_date",
        "assets",
        "durable_objects",
        "migrations",
    }
    assert (ROOT / "deploy" / "workers" / config["main"]).is_file()
    assert config["assets"] == {
        "binding": "ASSETS",
        "html_handling": "auto-trailing-slash",
        "not_found_handling": "404-page",
        "run_worker_first": ["/llm/*"],
    }
    # The site's own paths never start with /llm/, so the script never answers one of them.
    assert not (output / "llm").exists()
    assert (output / "404.html").is_file()
    assert builder["LIMIT_MIB"] <= 25
    for name in ("_headers", "_redirects", ".assetsignore", "_worker.js", "_routes.json"):
        assert not list(output.rglob(name)), name


def _get(url: str) -> tuple[int, str, bytes]:
    try:
        with urllib.request.urlopen(url, timeout=10) as response:
            return response.status, response.headers["Content-Type"], response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.headers["Content-Type"], error.read()


def test_the_local_server_answers_as_a_static_host(
    server_module: dict[str, Any], site: tuple[Path, bytes, dict[str, Any]]
) -> None:
    output, _, _ = site
    resolve = server_module["resolve"]
    assert resolve(output, "/") == output / "index.html"
    assert resolve(output, "/demo-v2?project=sample") == output / "demo-v2.html"
    assert resolve(output, "/demo-v2.css") == output / "demo-v2.css"
    for path in ("/../secret", "/browser/../../x", "/nope", "/browser/", "/demo-v2/"):
        assert resolve(output, path) is None, path
    server = server_module["start"](output)
    try:
        base = f"http://127.0.0.1:{server.server_port}"
        status, kind, body = _get(base + "/demo-v2?project=sample&run=run-x")
        assert (status, kind) == (200, "text/html; charset=utf-8")
        assert body == (output / "demo-v2.html").read_bytes()
        assert _get(base + "/browser/api_worker.mjs")[1] == "text/javascript; charset=utf-8"
        assert _get(base + "/browser/pkg/sheltersim_wasm_bg.wasm")[1] == "application/wasm"
        assert _get(base + "/sample/sample.json")[1] == "application/json; charset=utf-8"
        assert _get(base + "/THIRD-PARTY-NOTICES.txt")[1] == "text/plain; charset=utf-8"
        status, kind, body = _get(base + "/api/v1/projects")
        assert (status, kind) == (404, "text/html; charset=utf-8")
        assert body == (output / "404.html").read_bytes()
    finally:
        server.shutdown()
        server.server_close()
