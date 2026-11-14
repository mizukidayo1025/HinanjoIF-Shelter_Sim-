"""The Scenario text read with the LLM, as Python reads it, for the browser's port (Issue #421).

`parse_scenario_text(text, router=...)` asks the LLM for the stated fields (the task
`region-scenario-extraction-v1`), checks the answer against the task's schema, reads it back
(`_parse_llm_contract`) and merges it with the rules' values (`_merge_explicit`). The browser
site asks the same through its relay (deploy/workers/relay_tasks.mjs's `scenario_text`) and reads
the answer in the core (`sheltersim_core::scenario::text::parse_answer`).

`task_document` is the task as Python sends it: the model and options of the PREPARE stage, the
instruction the text follows, the system prompt (with the schema as Python writes it) and the
schema. `native/core/tools/scenario_text_task.py` writes it to
`native/core/src/scenario/scenario_text_task.json`, which the relay and the core read;
`tests/unit/llm/test_region_prepare.py` fails when that file is stale.

`answer_cases` are texts with LLM answers (valid ones, ones the schema refuses, ones
`_parse_llm_contract` refuses, conditions the browser does not compute) and what Python's parse
gives with a router whose provider answers that text (`python_answer`).
`native/core/tools/scenario_text_answers.py` writes them to
`native/core/tests/data/scenario_text_answers.json`, which `native/core/tests/scenario_text.rs`
compares the core with; the same test file checks it is current.
"""

from __future__ import annotations

import copy
import json
from typing import Any

TASK_NAME = "scenario_text"


class _Capture:
    """A router that keeps the task it is asked and answers nothing."""

    def __init__(self) -> None:
        self.records: list[Any] = []
        self.task: Any = None

    def complete(self, task: Any) -> str:
        self.task = task
        raise RuntimeError("captured")


def captured_task(text: str = "墨田区で地震。避難者10人。") -> Any:
    from sheltersim.llm.region_prepare import parse_scenario_text

    capture = _Capture()
    parse_scenario_text(text, router=capture)
    assert capture.task is not None
    return capture.task


def task_document() -> dict[str, Any]:
    """The task Python sends for a Scenario text, without the text."""
    from sheltersim.llm.config import LLMSettings
    from sheltersim.llm.contracts import LLMStage
    from sheltersim.llm.provider import _system_prompt

    text = "墨田区で地震。避難者10人。"
    task = captured_task(text)
    suffix = "\n\n" + text
    assert task.input_text.endswith(suffix)
    assert task.stage is LLMStage.PREPARE
    settings = LLMSettings()
    return {
        "task": TASK_NAME,
        "stage": task.stage.value,
        "model": settings.model_for(task.stage.value),
        # AnthropicProvider asks PREPARE tasks for a low effort (Opus cannot disable thinking).
        "effort": "low",
        "prompt_version": task.prompt_version,
        "schema_version": task.schema_version,
        "max_output_tokens": min(task.max_output_tokens, settings.max_output_tokens),
        "max_input_characters": 4000,
        # The user message is the instruction, a blank line and the text.
        "instruction": task.input_text[: -len(suffix)],
        "system": _system_prompt(task),
        "schema": task.output_schema,
    }


def task_text() -> str:
    """The text of `native/core/src/scenario/scenario_text_task.json`."""
    return json.dumps(task_document(), sort_keys=True, ensure_ascii=False, indent=1) + "\n"


def _contract(**changes: Any) -> dict[str, Any]:
    """`_to_llm_contract` of no values (every field empty or at its default), changed."""
    from sheltersim.llm.region_prepare import _to_llm_contract

    contract = copy.deepcopy(json.loads(json.dumps(_to_llm_contract({}))))
    for key, value in changes.items():
        contract[key] = value
    return contract


def _stock(**changes: Any) -> dict[str, Any]:
    conditions = _contract()["operational_conditions"]
    conditions["initial_resources"].update(changes)
    return conditions


def answer_cases() -> list[dict[str, str]]:
    """Texts and the LLM's answer to each: {name, text, output}."""
    sumida = "すみだで大きな地震。100人が避難。"
    full = "墨田区で地震。避難者40人、2日間。"
    cases: list[tuple[str, str, Any]] = [
        # Valid answers: what the rules missed comes from the answer.
        ("municipality-from-answer", sumida, _contract(municipality="墨田区", evacuees="100")),
        (
            "rules-win",
            full,
            _contract(municipality="横浜市", disaster="flood", evacuees="5", duration_days="9"),
        ),
        (
            "every-scalar",
            "大きな災害が起きた。",
            _contract(
                municipality="墨田区",
                disaster="earthquake",
                evacuees="120",
                duration_days="3.5",
                gross_area_m2="850",
                accessible_toilet_count="2",
                staff_count="6",
                seed="7",
                season="winter",
            ),
        ),
        ("unicode-digits", sumida, _contract(municipality="墨田区", evacuees="１００")),
        (
            "python-numbers",
            "墨田区で地震。",
            _contract(evacuees=" 1_000 ", duration_days="1e1", gross_area_m2="12_3.5"),
        ),
        ("float-days-text", "墨田区で地震。", _contract(evacuees="30", duration_days=" 2.50 ")),
        (
            "facility-conditions",
            full,
            _contract(facility_conditions=["ペット可", "授乳室がある"]),
        ),
        ("zero-people", "墨田区で地震。", _contract(evacuees="0")),
        (
            "stock-from-answer",
            full,
            _contract(
                operational_conditions=_stock(status="entered", water_ml=30000, food_meal_units=240)
            ),
        ),
        (
            "stock-merged",
            "墨田区で地震。避難者40人、2日間。食料200食。",
            _contract(operational_conditions=_stock(status="incomplete", water_ml=5000)),
        ),
        (
            "stock-rules-win",
            "墨田区で地震。避難者40人、2日間。飲料水300L、食料600食。",
            _contract(
                operational_conditions=_stock(status="entered", water_ml=1, food_meal_units=2)
            ),
        ),
        # Answers the schema refuses: the router answers with the rules' fallback.
        ("not-json", full, "地震、40人"),
        ("fenced", full, "```json\n" + json.dumps(_contract(evacuees="40")) + "\n```"),
        ("empty", full, "   "),
        ("missing-field", full, {k: v for k, v in _contract().items() if k != "season"}),
        ("extra-field", full, _contract(note="x")),
        ("number-not-string", full, _contract(evacuees=40)),
        ("too-long", full, _contract(evacuees="1" * 13)),
        (
            "nan-constant",
            full,
            json.dumps(_contract()).replace(
                '"zone_area_shares": {}', '"zone_area_shares": {"a": NaN}'
            ),
        ),
        ("share-above-one", full, _contract(zone_area_shares={"sleeping": 2})),
        ("not-an-object", full, "[]"),
        # Answers `_parse_llm_contract` refuses: the rules' values alone.
        ("people-not-a-number", "墨田区で地震。", _contract(evacuees="many")),
        ("blank-seed", full, _contract(seed=" ")),
        ("days-not-a-number", full, _contract(duration_days="2日")),
        ("zone-invalid", full, _contract(facility_zones=[{"kind": "x"}])),
        # What the request then refuses (Python's 400).
        ("season-not-canonical", full, _contract(season="冬")),
        ("zero-days", "墨田区で地震。避難者40人。", _contract(duration_days="0")),
        ("too-many-people", "墨田区で地震。", _contract(evacuees="999999999")),
        # Conditions the browser does not compute (the core: unsupported).
        ("other-warehouse", full, _contract(operational_conditions=_stock(warehouse_id="-x"))),
        (
            "population-condition",
            full,
            _contract(
                population_conditions={
                    **_contract()["population_conditions"],
                    "pet_household_rate": 0.2,
                }
            ),
        ),
        ("zone-shares", full, _contract(zone_area_shares={"sleeping": 0.5})),
    ]
    out = []
    for name, text, output in cases:
        if not isinstance(output, str):
            output = json.dumps(output, ensure_ascii=False)
        out.append({"name": name, "text": text, "output": output})
    return out


class _Provider:
    def __init__(self, output: str) -> None:
        self.output = output

    def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
        from sheltersim.llm.contracts import ProviderResponse

        return ProviderResponse(
            text=self.output, model_id=model_id, input_tokens=10, output_tokens=4, elapsed_ms=1.0
        )


def python_answer(text: str, output: str) -> dict[str, Any]:
    """What Python's parse gives when the LLM answers `output`: {"kind": "ok", "value": the
    parse} or {"kind": "error"} (a ValueError, which the server answers with 400)."""
    from sheltersim.llm.config import LLMMode, LLMSettings
    from sheltersim.llm.provider import LLMRouter, ResponseTape
    from sheltersim.llm.region_prepare import parse_scenario_text

    router = LLMRouter(
        LLMSettings(mode=LLMMode.RECORD, api_key="test-only-key", budget_usd=100.0),
        provider=_Provider(output),
        tape=ResponseTape(),
    )
    try:
        parsed = parse_scenario_text(text, router=router)
    except ValueError:
        return {"kind": "error"}
    request = None if parsed.request is None else parsed.request.model_dump(mode="json")
    if request is not None:
        request["field_origins"] = dict(sorted(request["field_origins"].items()))
    return {
        "kind": "ok",
        "value": {
            "request": request,
            "missing_fields": list(parsed.missing_fields),
            "source": parsed.source,
            "warnings": list(parsed.warnings),
            "input_sha256": parsed.input_sha256,
        },
    }


def answers_document() -> dict[str, Any]:
    cases = []
    for case in answer_cases():
        cases.append({**case, "python": python_answer(case["text"], case["output"])})
    return {"task": task_document()["prompt_version"], "cases": cases}


def answers_text() -> str:
    """The text of `native/core/tests/data/scenario_text_answers.json`."""
    return json.dumps(answers_document(), sort_keys=True, ensure_ascii=False, indent=1) + "\n"
