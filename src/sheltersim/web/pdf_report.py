"""The report of a saved Project Run (#463): the core builds its data and its facts and, where the
request asks for it (`"analysis": true`, #582), the analysis asked of Opus, and checks the analysis;
this module gives the core each branch's Run, asks the configured router through the core's port
and keeps the answer with the analysis.

The answer is the core's JSON of the data, the facts and the checked analysis, or `null` where
none is asked (`PdfPlan.report`, `PdfAnalysis.report`); the screen lays it out on its print page,
which the user prints or saves as a PDF (#470: no PDF is typeset here, as the browser has no
typesetter). The data and the facts are the core's (`sheltersim_core::answers::pdf`); no evidence
is built here. A report without the analysis asks no LLM and is not kept: it is made again from the
stored Runs each time, and a kept report is never answered for it. An analysis asked without an API
key is the core's refusal (409 `pdf_analysis_offline`), as the browser's. An answer the core's
checks refuse is asked once more with the reason (`PdfAnalysis.retry`, #608). A report with the
analysis kept for the same identity (the branches' heads, the Scenario, the minute, the versions
and the Opus model) is answered again without asking anyone.
"""

from __future__ import annotations

import base64
import json
import threading
from dataclasses import replace
from typing import Any

from sheltersim.storage.projects import StorageError, digest, safe_document

REPORT_VERSION = "pdf-report-ja-v1"
PROMPT_VERSION = "pdf-analysis-ja-v2"
# The kept report's form: 2 holds the route's answer (#470); 1 held a typeset PDF and is asked
# again.
KEPT_VERSION = 2


class PdfReports:
    """`POST runs/{run}/pdf-report`: one export at a time in this process."""

    def __init__(self, api: Any) -> None:
        self.api = api
        self._lock = threading.Lock()

    def export(self, project_id: str, run_id: str, body: bytes) -> bytes:
        """The report's JSON (the core's `PdfPlan.report` without the analysis, `PdfAnalysis.report`
        with it), or the refusal (the core's `Refused`, or a StorageError)."""
        from sheltersim.llm.config import LLMSettings
        from sheltersim.llm.link import analysis_link
        from sheltersim.web.projects import answers

        core, files, store = answers(), self.api.files, self.api.store
        listing = files.run_listing(project_id, run_id)
        plan = core.pdf_plan(project_id, run_id, body, *listing)
        if not plan.analysis_asked:
            # The core's data and facts alone: no LLM is asked, and nothing is kept.
            heads = json.loads(plan.records_json())
            with self._lock:
                return plan.report(self._add_branches(project_id, run_id, plan, heads))
        settings = replace(LLMSettings.from_env(), review_model_family="opus")
        link = analysis_link(
            store,
            project_id,
            run_id,
            operation="pdf_report",
            branch_id=plan.branch_ids[-1],
            settings=settings,
        )
        # Without an API key: the core's refusal, the browser's.
        plan.check_link(link is not None)
        if not settings.model_for("review").startswith("claude-opus-"):
            raise StorageError("pdf_opus_required: PDF分析にはOpusを設定してください。")
        with self._lock:
            return self._export(project_id, run_id, plan, link, settings)

    def _export(self, project_id: str, run_id: str, plan: Any, link: Any, settings: Any) -> bytes:
        from sheltersim.llm.link import ask
        from sheltersim.web.projects import answers

        core, files, store = answers(), self.api.files, self.api.store
        run = json.loads(files.run_file(project_id, run_id, "run.json"))
        heads = json.loads(plan.records_json())
        identity = {
            "heads": heads,
            "scenario_hash": run["scenario_hash"],
            "minute": plan.minute,
            "version": REPORT_VERSION,
            "prompt_version": PROMPT_VERSION,
            "model": settings.opus_model,
        }
        key = digest(identity)
        path = store._path(project_id, "runs", run_id, "pdf-reports", f"{key}.json")
        if path.exists():
            saved = store._read(path)
            encoded = saved.get("answer_base64")
            if (
                saved.get("schema_version") == KEPT_VERSION
                and saved.get("identity") == identity
                and isinstance(encoded, str)
                and digest(encoded) == saved.get("answer_hash")
            ):
                try:
                    return base64.b64decode(encoded, validate=True)
                except ValueError:
                    pass
            # Another form, or a damaged answer: asked again (the Run's tape answers it).
        scenario = self._add_branches(project_id, run_id, plan, heads)
        attempt_path = store._path(project_id, "runs", run_id, "pdf-attempts", f"{key}.json")
        attempt = store._read(attempt_path)["next_attempt"] if attempt_path.exists() else 0
        analysis = plan.analysis(scenario, attempt)
        reply = ask(link, analysis.task())
        # Once more, told why, where the checks refused Opus's answer (#608).
        again = analysis.retry(reply)
        if again is not None:
            analysis, reply = again, ask(link, again.task())
        try:
            checked = analysis.read(reply)
        except core.Refused:
            record = json.loads(reply) if reply is not None else {}
            if record.get("api_request_count") or record.get("cache_hit"):
                # A retry after an answer that was paid for and refused is a new request; the
                # receipts of the earlier one stay.
                with store.transaction():
                    store._write(attempt_path, {"schema_version": 1, "next_attempt": attempt + 1})
            raise
        answer = analysis.report(checked)
        now = core.pdf_plan(
            project_id,
            run_id,
            json.dumps({"branch_ids": plan.branch_ids, "minute": plan.minute}).encode(),
            *files.run_listing(project_id, run_id),
        )
        if [item["checkpoint_hash"] for item in json.loads(now.records_json())] != [
            item["checkpoint_hash"] for item in heads
        ]:
            raise StorageError("pdf_scope_changed: 計算結果が更新されました。再試行してください。")
        # As base64: the answer's bytes kept as they are (the store redacts what looks private in
        # its strings).
        encoded = base64.b64encode(answer).decode("ascii")
        artifact = {
            "schema_version": KEPT_VERSION,
            "identity": identity,
            "answer_hash": digest(encoded),
            "answer_base64": encoded,
        }
        with store.transaction():
            store._write(path, safe_document(artifact))
        return answer

    def _add_branches(self, project_id: str, run_id: str, plan: Any, heads: list[Any]) -> bytes:
        """Give the core each branch's Run at the minute, its frame, its KPI table and its
        journal; the Run's Scenario."""
        from sheltersim.web.projects import answers

        core, files, api = answers(), self.api.files, self.api
        scenario = files.run_scenario_bytes(project_id, run_id)
        for record in heads:
            branch_id = record["branch_id"]
            journal = files.branch_file(project_id, run_id, branch_id, "journal.jsonl") or b""
            exact = False
            if journal and core.view_only(journal, scenario):
                # A Run of other model versions is read at its saved points only (D2).
                points = json.loads(files.reader(project_id, run_id, branch_id).heads_json())
                saved = sorted(
                    head["minute"]
                    for head in points
                    if head["kind"] != "end" and head["minute"] <= plan.minute
                )
                plan.check_saved_point(saved[-1] if saved else 0)
                exact = True
            branch_run = api.live.at(project_id, run_id, record, plan.minute, exact=exact)
            frame = branch_run.frame(run_id, branch_id, record["computed_until"])
            plan.add_branch(branch_run, frame, api.samples(project_id, run_id, branch_id), journal)
        return scenario
