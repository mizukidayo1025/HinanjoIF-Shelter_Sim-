"""The browser site's LLM relay (deploy/workers/relay.mjs, #419) run in node for tests:
`run_relay` hands a script to tests/support/llm_relay_harness.mjs (its Durable Object on
node:sqlite, Anthropic replaced by the script's answers) and reads what it answered. Skipped
without a node that has node:sqlite (22.5 or later).
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path
from typing import Any

import pytest

HARNESS = Path(__file__).parent / "llm_relay_harness.mjs"


def relay_node() -> str:
    """The node that runs the relay, or the test is skipped."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is required for the relay's tests")
    probe = subprocess.run(
        [node, "-e", "require('node:sqlite')"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if probe.returncode != 0:
        pytest.skip("the relay's tests need node:sqlite (node 22.5 or later)")
    return node


def run_relay(script: dict[str, Any]) -> dict[str, Any]:
    """What the relay answered to the script's steps (the harness's stdin and stdout)."""
    done = subprocess.run(
        [relay_node(), str(HARNESS)],
        input=json.dumps(script, ensure_ascii=False),
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
        timeout=60,
    )
    return json.loads(done.stdout)


# The passphrase `relay_requests` presents, and the recipients it is one of.
_PASSPHRASE = "abcde-fghjk-mnpqr-stuvw"
_ANSWER = {
    "model": "claude-opus-5-5",
    "stop_reason": "end_turn",
    "content": [{"type": "text", "text": "{}"}],
    "usage": {"input_tokens": 100, "output_tokens": 50},
}


def relay_requests(tasks: list[bytes]) -> list[dict[str, Any]]:
    """What the relay sends Anthropic for each core task (`llm::Task`'s JSON, as the browser
    sends it in `input`, #470): the request's body. Each task must be let in."""
    import hashlib

    entry = {"label": "テスト", "sha256": hashlib.sha256(_PASSPHRASE.encode()).hexdigest()}
    steps = [
        {
            "now": 0,
            "request": {
                "path": f"/llm/v1/tasks/{json.loads(task)['task']}",
                "headers": {
                    "Authorization": f"Bearer {_PASSPHRASE}",
                    "Content-Type": "application/json",
                },
                "body": '{"input":' + task.decode() + "}",
            },
            "provider": {"status": 200, "body": _ANSWER},
        }
        for task in tasks
    ]
    done = run_relay(
        {
            "env": {"ANTHROPIC_API_KEY": "sk-test", "LLM_PASSPHRASES": json.dumps([entry])},
            "real_tasks": True,
            "steps": steps,
        }
    )
    for answer in done["answers"]:
        assert answer["status"] == 200, answer
    return [answer["provider_request"]["body"] for answer in done["answers"]]


def router_request(*, model_id: str, task: Any, max_output_tokens: int) -> dict[str, Any]:
    """The parts of the request Python's AnthropicProvider sends for a task of the analysis or
    intervention stage (src/sheltersim/llm/provider.py), as the relay writes them."""
    from sheltersim.llm.provider import _system_prompt

    return {
        "model": model_id,
        "max_tokens": max_output_tokens,
        "system": _system_prompt(task),
        "messages": [{"role": "user", "content": task.input_text}],
    }
