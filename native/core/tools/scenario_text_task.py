"""Write the task the browser site asks the LLM to read a Scenario text with (Issue #421).

    SHELTERSIM_LLM_MODE=offline python native/core/tools/scenario_text_task.py

`native/core/src/scenario/scenario_text_task.json` gets `tests/support/core_scenario_text.py`'s
`task_document`: the task `parse_scenario_text` sends (its model and options, instruction, system
prompt and schema), which the relay (deploy/workers/relay_tasks.mjs) sends and the core
(`sheltersim_core::scenario::text::parse_answer`) checks answers against; and
`native/core/tests/data/scenario_text_answers.json` gets its LLM answers with what Python's parse
makes of each. `tests/unit/llm/test_region_prepare.py` fails when either file is stale.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TASK = ROOT / "native" / "core" / "src" / "scenario" / "scenario_text_task.json"
ANSWERS = ROOT / "native" / "core" / "tests" / "data" / "scenario_text_answers.json"


def main() -> None:
    sys.path.insert(0, str(ROOT / "tests"))
    from support.core_scenario_text import answers_text, task_text

    for path, text in ((TASK, task_text()), (ANSWERS, answers_text())):
        path.write_text(text, encoding="utf-8")
        print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
