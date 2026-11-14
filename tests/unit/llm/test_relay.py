"""The LLM relay of the browser version (deploy/workers/relay.mjs, #419), run in node by
tests/support/llm_relay_harness.mjs with its Durable Object on node:sqlite and Anthropic replaced
by fixed answers: only holders of a passphrase it gave out are let in, too many wrong ones from an
address are refused for a while, the whole site's spending stays under the daily cap (reserved
before a call, settled after it, a new day at 00:00 Japan time), the prompt is the task's, and
neither the passphrase nor the API key is ever answered or logged. Skipped without a node that has
node:sqlite.
"""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import subprocess
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest

from sheltersim.llm.config import _OFFICIAL_MODEL_PRICE_FLOORS

ROOT = Path(__file__).parents[3]
RELAY = ROOT / "deploy" / "workers" / "relay.mjs"
HARNESS = ROOT / "tests" / "support" / "llm_relay_harness.mjs"
NEW_PASSPHRASE = ROOT / "deploy" / "workers" / "new_passphrase.mjs"
KEY = "sk-ant-test-key-never-shown"
PASSPHRASE = "abcde-fghjk-mnpqr-stuvw"
OTHER = "zzzzz-yyyyy-xxxxx-wwwww"
RECIPIENTS = json.dumps(
    [
        {"label": "審査員A", "sha256": hashlib.sha256(PASSPHRASE.encode()).hexdigest()},
        {"label": "審査員B", "sha256": hashlib.sha256(OTHER.encode()).hexdigest()},
        {"label": "壊れた項目", "sha256": "not-a-digest"},
    ],
    ensure_ascii=False,
)
# 23:59 on 2026-11-15 in Japan, and the next day's 00:00.
LATE = int(datetime(2026, 11, 15, 14, 59, tzinfo=UTC).timestamp() * 1000)
MIDNIGHT = LATE + 60_000
TASK = {
    "model": "claude-opus-5-5",
    "promptVersion": "test-v1",
    "maxOutputTokens": 1000,
    "maxPromptBytes": 4000,
    "effort": "low",
    "rejectInput": True,
}
USAGE = {
    "input_tokens": 100,
    "output_tokens": 50,
    "cache_read_input_tokens": 0,
    "cache_creation_input_tokens": 0,
}
ANSWER = {
    "model": "claude-opus-5-5",
    "stop_reason": "end_turn",
    "content": [{"type": "thinking", "thinking": "…"}, {"type": "text", "text": '{"ok": '}]
    + [{"type": "text", "text": "true}"}],
    "usage": USAGE,
}


def _node() -> str:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is required for the relay's tests")
    probe = subprocess.run(
        [node, "-e", "require('node:sqlite')"], capture_output=True, text=True, check=False
    )
    if probe.returncode != 0:
        pytest.skip("the relay's tests need node:sqlite (node 22.5 or later)")
    return node


def _run(steps: list[dict[str, Any]], *, env: dict[str, Any] | None = None, **tasks: Any) -> Any:
    script = {
        "env": {"ANTHROPIC_API_KEY": KEY, "LLM_PASSPHRASES": RECIPIENTS} if env is None else env,
        "tasks": tasks or {"probe": TASK},
        "steps": steps,
    }
    done = subprocess.run(
        [_node(), str(HARNESS)],
        input=json.dumps(script, ensure_ascii=False),
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    # Neither secret ever leaves the relay: not in an answer, not in a log line.
    assert KEY not in done.stdout and PASSPHRASE not in done.stdout and OTHER not in done.stdout
    return json.loads(done.stdout)


def _post(path: str, passphrase: str | None = PASSPHRASE, **step: Any) -> dict[str, Any]:
    headers = {"Content-Type": "application/json"}
    if passphrase is not None:
        headers["Authorization"] = f"Bearer {passphrase}"
    headers |= step.pop("headers", {})
    request = {"path": path, "headers": headers, "body": step.pop("body", {})}
    return {"now": step.pop("now", LATE), "request": request | step.pop("request", {}), **step}


def _task(input_text: str = "避難所の条件", **step: Any) -> dict[str, Any]:
    step.setdefault("provider", {"status": 200, "body": ANSWER})
    return _post("/llm/v1/tasks/probe", body={"input": input_text}, **step)


def test_the_relay_s_prices_are_python_s() -> None:
    text = RELAY.read_text(encoding="utf-8")
    block = text[text.index("export const PRICES") : text.index("});", text.index("PRICES"))]
    prices = {
        model: (float(a), float(b))
        for model, a, b in re.findall(
            r'"([^"]+)": \{ input: ([0-9.]+), output: ([0-9.]+) \}', block
        )
    }
    assert prices == {
        model: (price.input_usd_per_million, price.output_usd_per_million)
        for model, price in _OFFICIAL_MODEL_PRICE_FLOORS.items()
    }


def test_a_passphrase_given_out_is_let_in_and_others_are_not() -> None:
    result = _run(
        [
            _post("/llm/v1/session"),
            _post("/llm/v1/session", passphrase=OTHER),
            _post("/llm/v1/session", passphrase="wrong"),
            _post("/llm/v1/session", passphrase=None),
        ]
    )
    first, second, wrong, missing = result["answers"]
    assert first["status"] == 200 and first["type"] == "application/json; charset=utf-8"
    assert first["body"] == {
        "recipient": "審査員A",
        "day": "2026-11-15",
        "cap_usd": 5,
        "remaining_usd": 5,
        "available": True,
    }
    assert second["body"]["recipient"] == "審査員B"
    assert (wrong["status"], wrong["body"]["code"]) == (401, "llm_passphrase_invalid")
    assert wrong["body"]["error"] == "合言葉が違います。"
    assert (missing["status"], missing["body"]["code"]) == (401, "llm_passphrase_required")
    # Only the wrong one counts against the address; the malformed entry is left out, said so.
    assert [row["count"] for row in result["ledger"]["failures"]] == [1]
    assert any("llm_relay_passphrase_entry_skipped" in line for line in result["logs"])


def test_without_an_api_key_the_session_says_so_and_tasks_are_refused() -> None:
    result = _run(
        [_post("/llm/v1/session"), _task()],
        env={"LLM_PASSPHRASES": RECIPIENTS},
    )
    session, task = result["answers"]
    assert session["body"]["available"] is False
    assert (task["status"], task["body"]["code"]) == (503, "llm_relay_unconfigured")
    assert "provider_request" not in task


def test_wrong_passphrases_from_one_address_lock_it_out_for_the_window() -> None:
    address = {"CF-Connecting-IP": "203.0.113.7"}
    steps = [_post("/llm/v1/session", passphrase="wrong", headers=address) for _ in range(10)]
    steps += [
        _post("/llm/v1/session", headers=address),
        _post("/llm/v1/session", headers={"CF-Connecting-IP": "203.0.113.8"}),
        _post("/llm/v1/session", headers=address, now=LATE + 15 * 60_000),
    ]
    answers = _run(steps)["answers"]
    assert {a["status"] for a in answers[:10]} == {401}
    locked, elsewhere, later = answers[10:]
    assert (locked["status"], locked["body"]["code"]) == (429, "llm_passphrase_locked")
    assert elsewhere["status"] == 200 and later["status"] == 200


def test_requests_that_are_not_the_relay_s_are_refused_and_other_paths_are_the_site_s() -> None:
    answers = _run(
        [
            _post("/llm/v1/session", headers={"Origin": "https://evil.example"}),
            _post("/llm/v1/session", headers={"Sec-Fetch-Site": "cross-site"}),
            _post("/llm/v1/session", headers={"Origin": "https://hinanjo-if.example"}),
            _post("/llm/v1/session", request={"method": "GET"}),
            _post("/llm/v1/session", body="x" * (64 * 1024 + 1)),
            _post("/llm/v1/session", body="[1, 2]"),
            _post("/llm/v1/nothing"),
            _post("/demo-v2.css", request={"method": "GET"}),
        ]
    )["answers"]
    codes = [
        (a["status"], a["body"].get("code") if isinstance(a["body"], dict) else a["body"])
        for a in answers
    ]
    assert codes == [
        (403, "llm_cross_origin"),
        (403, "llm_cross_origin"),
        (200, None),  # the site's own origin
        (405, "llm_method_not_allowed"),
        (413, "llm_body_too_large"),
        (400, "llm_body_invalid"),
        (404, "llm_route_unknown"),
        (200, "asset"),
    ]


def test_a_task_builds_its_prompt_reserves_calls_and_settles_at_its_usage() -> None:
    result = _run([_task()])
    (answer,) = result["answers"]
    sent = answer["provider_request"]
    assert sent["url"] == "https://api.anthropic.com/v1/messages"
    assert sent["headers"]["x-api-key"] == "<the env's key>"
    assert sent["headers"]["anthropic-version"] == "2023-06-01"
    # The prompt is the task's; Opus gets thinking's room on top of the answer (Python's 4,096).
    assert sent["body"] == {
        "model": "claude-opus-5-5",
        "max_tokens": 1000 + 4096,
        "system": "test system",
        "messages": [{"role": "user", "content": "避難所の条件"}],
        "output_config": {"effort": "low"},
    }
    cost_micro = 100 * 4 + 50 * 20  # tokens × USD per million tokens
    assert answer["status"] == 200
    assert answer["body"] == {
        "task": "probe",
        "prompt_version": "test-v1",
        "model": "claude-opus-5-5",
        "output_text": '{"ok": true}',
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 100, "output_tokens": 50, "cost_usd": cost_micro / 1e6},
        "remaining_usd": 5 - cost_micro / 1e6,
    }
    ledger = result["ledger"]
    assert ledger["reservations"] == []
    assert ledger["spend"] == [{"day": "2026-11-15", "spent_micro": cost_micro}]
    assert ledger["usage"] == [
        {
            "day": "2026-11-15",
            "recipient": "審査員A",
            "calls": 1,
            "spent_micro": cost_micro,
            "input_tokens": 100,
            "output_tokens": 50,
        }
    ]
    (line,) = [json.loads(line) for line in result["logs"] if "llm_relay_call" in line]
    assert line == {
        "event": "llm_relay_call",
        "recipient": "審査員A",
        "task": "probe",
        "outcome": "answered",
        "cost_usd": cost_micro / 1e6,
    }


def test_tasks_refuse_what_they_cannot_build_and_the_unknown() -> None:
    answers = _run(
        [
            _post("/llm/v1/tasks/nothing", body={"input": "x"}),
            _task("bad"),
            _task("長" * 2000),
        ]
    )["answers"]
    assert [(a["status"], a["body"]["code"]) for a in answers] == [
        (404, "llm_task_unknown"),
        (400, "llm_task_input_invalid"),
        (413, "llm_task_input_too_large"),
    ]
    assert all("provider_request" not in a for a in answers)


def test_the_daily_cap_counts_reservations_and_comes_back_at_midnight_in_japan() -> None:
    # The first call costs $4 (200,000 output tokens on Opus); the next one's reservation
    # (its prompt and up to 50,000 + 4,096 tokens, about $1.08) would pass $5.
    costly = ANSWER | {"usage": USAGE | {"input_tokens": 0, "output_tokens": 200_000}}
    big = TASK | {"maxOutputTokens": 50_000}
    result = _run(
        [
            _task(provider={"status": 200, "body": costly}),
            _task(),
            _post("/llm/v1/session"),
            _task(now=MIDNIGHT),
        ],
        probe=big,
    )
    first, refused, session, next_day = result["answers"]
    assert first["body"]["remaining_usd"] == 1
    assert (refused["status"], refused["body"]["code"]) == (429, "llm_budget_exceeded")
    assert refused["body"]["remaining_usd"] == 1 and "provider_request" not in refused
    assert "$5" in refused["body"]["error"] and "0時" in refused["body"]["error"]
    assert session["body"]["remaining_usd"] == 1
    assert next_day["status"] == 200
    assert [row["day"] for row in result["ledger"]["spend"]] == ["2026-11-15", "2026-11-16"]


def test_a_single_call_above_the_cap_is_never_sent() -> None:
    answers = _run([_task()], probe=TASK | {"maxOutputTokens": 300_000})["answers"]
    assert answers[0]["status"] == 429 and "provider_request" not in answers[0]


def test_failed_calls_settle_at_nothing_or_at_their_reservation() -> None:
    result = _run(
        [
            _task(provider={"status": 500, "body": {"error": "x"}}),
            _task(provider={"status": 529, "body": {"error": "overloaded"}}),
            _task(provider="unreachable"),
        ]
    )
    error, busy, unreachable = result["answers"]
    assert (error["status"], error["body"]["code"]) == (502, "llm_provider_error")
    assert (busy["status"], busy["body"]["code"]) == (503, "llm_provider_busy")
    assert (unreachable["status"], unreachable["body"]["code"]) == (502, "llm_provider_unreachable")
    # The unanswered call may have been billed: it counts at its reservation (the prompt's
    # bytes + 256 at twice the input price, 5,096 tokens at the output price).
    prompt_bytes = len(
        json.dumps(
            {"system": "test system", "messages": [{"role": "user", "content": "避難所の条件"}]},
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode()
    )
    reserved = (prompt_bytes + 256) * 4 * 2 + 5096 * 20
    assert result["ledger"]["spend"] == [{"day": "2026-11-15", "spent_micro": reserved}]
    assert result["ledger"]["reservations"] == []
    assert [row["calls"] for row in result["ledger"]["usage"]] == [3]


def test_new_passphrase_makes_an_entry_the_relay_lets_in() -> None:
    made = subprocess.run(
        [_node(), str(NEW_PASSPHRASE), "審査員C"], capture_output=True, text=True, check=True
    ).stdout
    passphrase = re.search(r"^passphrase: (\S+)$", made, re.M).group(1)  # type: ignore[union-attr]
    entry = json.loads(re.search(r"^entry: +(.+)$", made, re.M).group(1))  # type: ignore[union-attr]
    assert re.fullmatch(r"[a-z2-9]{5}(-[a-z2-9]{5}){3}", passphrase)
    assert entry == {"label": "審査員C", "sha256": hashlib.sha256(passphrase.encode()).hexdigest()}
    script = {
        "env": {"LLM_PASSPHRASES": json.dumps([entry])},
        "steps": [_post("/llm/v1/session", passphrase=passphrase)],
    }
    done = subprocess.run(
        [_node(), str(HARNESS)],
        input=json.dumps(script),
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(done.stdout)["answers"][0]["body"]["recipient"] == "審査員C"
