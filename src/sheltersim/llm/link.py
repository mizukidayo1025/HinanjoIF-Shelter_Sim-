"""Python's LLM router behind the core's port to an LLM (#428).

The core asks an LLM during a tick through one method, `LlmLink::complete(task) -> reply`
(`native/core/src/llm.rs`): its rules (`rules/llm_runtime.rs`) build the prompt, read the answer
against its schema and write the world themselves. `RouterLink` is that link for the PyO3
`PlainRun` (`PlainRun.set_llm`): it turns the core's task into an `LLMTask`, lets an `LLMRouter`
answer it (its budget and call limits, response tape, usage ledger observer, or a `RecordedRouter`'s
check of the recorded order), and gives back the text with the router's record of the call.

The answers made with an LLM (#463: a result question, the review of a completed branch, the
reading of an intervention's text, the PDF report's analysis) ask through the same link: the core
builds the task and checks the reply (`sheltersim_core::answers::asking`); `ask` hands the task to
the link and gives the reply back, or None where the router raised.
"""

from __future__ import annotations

import json
from typing import Any

from sheltersim.llm.config import LLMMode, LLMSettings
from sheltersim.llm.contracts import LLMStage, LLMTask
from sheltersim.llm.provider import LLMRouter

# The accepted replies of a Run's answers made with an LLM, by the router's cache key: the
# analysis (questions, review, PDF) and the intervention compiler.
ANALYSIS_TAPE = "analysis-tape.json"
COMPILER_TAPE = "compiler-tape.json"


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


def analysis_link(
    store: Any,
    project_id: str,
    run_id: str,
    *,
    operation: str,
    branch_id: str,
    settings: LLMSettings | None = None,
    tape: str = ANALYSIS_TAPE,
) -> RouterLink | None:
    """The link an answer made with an LLM asks through (#463): the configured router behind
    the core's port, its calls counted in the Run's usage account and its accepted replies kept
    in the Run's `tape` (`analysis-tape.json`; the intervention compiler's `compiler-tape.json`), so
    that a replay mode answers them again without a Provider. None without an API key (the
    answers without an LLM)."""
    from sheltersim.llm.accounting import StoredResponseTape, accounted_router

    settings = settings or LLMSettings.from_env()
    if settings.effective_mode is LLMMode.OFFLINE:
        return None
    stored = StoredResponseTape(store, store._path(project_id, "runs", run_id, tape))
    router = accounted_router(
        store,
        project_id,
        run_id=run_id,
        operation=operation,
        branch_id=branch_id,
        settings=settings,
        tape=stored,
    )
    return RouterLink(router)


def ask(link: RouterLink, task: bytes) -> bytes | None:
    """The link's reply to the core's task, or None where the router raised (a budget that
    stops, an allowance refused, a provider error under a stop policy): the core then answers
    as for a failed call."""
    try:
        return link.complete(task)
    except Exception:
        return None


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
        input_byte_limit=document.get("input_byte_limit"),
    )
