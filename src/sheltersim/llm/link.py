"""Python's LLM router behind the core's port to an LLM (#428).

The core asks an LLM during a tick through one method, `LlmLink::complete(task) -> reply`
(`native/core/src/llm.rs`): its rules (`rules/llm_runtime.rs`) build the prompt, read the answer
against its schema and write the world themselves. `RouterLink` is that link for the PyO3
`PlainRun` (`PlainRun.set_llm`): it turns the core's task into an `LLMTask`, lets an `LLMRouter`
answer it (its budget and call limits, response tape, usage ledger observer, or a `RecordedRouter`'s
check of the recorded order), and gives back the text with the router's record of the call.
"""

from __future__ import annotations

import json
from typing import Any

from sheltersim.llm.config import LLMMode
from sheltersim.llm.contracts import LLMStage, LLMTask
from sheltersim.llm.provider import LLMRouter


class RouterLink:
    """The core's `LlmLink` as an `LLMRouter`: `offline` and `complete(task: bytes) -> bytes`."""

    def __init__(self, router: LLMRouter) -> None:
        self.router = router

    @property
    def offline(self) -> bool:
        """Whether the router answers every task with its fallback: the core then asks nothing."""
        return self.router.mode is LLMMode.OFFLINE

    def complete(self, task: bytes) -> bytes:
        """The router's answer to the core's task (`llm::Task`'s JSON), as `llm::Reply`'s JSON."""
        asked = llm_task(json.loads(task))
        before = len(self.router.records)
        text = self.router.complete(asked)
        if len(self.router.records) <= before:
            raise RuntimeError("the LLM router answered without recording the call")
        record = self.router.records[-1]
        reply: dict[str, Any] = {
            "text": text,
            "outcome": record.outcome,
            "fallback_used": record.fallback_used,
            "cache_hit": record.cache_hit,
            "model_id": record.model_id,
            "estimated_cost_usd": float(record.estimated_cost_usd),
            "api_request_count": record.api_request_count,
            "input_tokens": record.input_tokens,
            "output_tokens": record.output_tokens,
        }
        return json.dumps(reply, ensure_ascii=False, separators=(",", ":")).encode()


def llm_task(document: dict[str, Any]) -> LLMTask:
    """The `LLMTask` of the core's task: the same input, state hash, versions, schema and fallback,
    so the router's cache key and a recorded call's identity are the core's."""
    return LLMTask(
        stage=LLMStage(document["stage"]),
        input_text=document["input_text"],
        state_hash=document["state_hash"],
        prompt_version=document["prompt_version"],
        schema_version=document["schema_version"],
        output_schema=document["output_schema"],
        fallback_text=document["fallback_text"],
        max_output_tokens=document["max_output_tokens"],
    )
