"""The PDF report of a saved Project Run (#463): the core builds its data, its facts and the
analysis asked of Opus, and checks the analysis; this module gives the core each branch's Run,
asks the configured router through the core's port, typesets the PDF (`pdf_render`) and keeps it.

The data and the facts are the core's (`sheltersim_core::answers::pdf`); no evidence is built
here. Without an API key the route answers the core's refusal (409 `pdf_analysis_offline`), as the
browser does. A report kept for the same identity (the branches' heads, the Scenario, the minute,
the versions and the Opus model) is answered again without asking anyone.
"""

from __future__ import annotations

import base64
import json
import math
import threading
from dataclasses import replace
from typing import Any

from sheltersim.storage.projects import StorageError, digest, safe_document

REPORT_VERSION = "pdf-report-ja-v1"
PROMPT_VERSION = "pdf-analysis-ja-v1"
# key, Japanese label, displayed unit, scale, lower-is-better (None = descriptive): the core's
# `pdf::METRICS`, which the renderer prints.
METRICS = (
    ("stress_mean", "平均ストレス", "/100", 100, True),
    ("toilet_wait_max_minutes", "最大トイレ待ち時間", "分", 1, True),
    ("staff_load_ratio", "スタッフ稼働率", "%", 100, None),
    ("unresolved_support_request_count", "未解決の支援要請", "件", 1, True),
    ("toilet_queue_length", "トイレ待ち人数", "人", 1, True),
    ("stress_high_count", "高ストレス者数", "人", 1, True),
)


def number(value: Any) -> bool:
    return type(value) in (int, float) and math.isfinite(value)


class PdfReports:
    """`POST runs/{run}/pdf-report`: one export at a time in this process."""

    def __init__(self, api: Any) -> None:
        self.api = api
        self._lock = threading.Lock()

    def export(self, project_id: str, run_id: str, body: bytes) -> bytes:
        """The PDF's bytes, or the refusal (the core's `Refused`, or a StorageError)."""
        from sheltersim.llm.config import LLMSettings
        from sheltersim.llm.link import analysis_link
        from sheltersim.web.projects import answers

        core, files, store = answers(), self.api.files, self.api.store
        listing = files.run_listing(project_id, run_id)
        settings = replace(LLMSettings.from_env(), review_model_family="opus")
        plan = core.pdf_plan(project_id, run_id, body, *listing)
        link = analysis_link(
            store,
            project_id,
            run_id,
            operation="pdf_report",
            branch_id=plan.branch_ids[-1],
            settings=settings,
        )
        if link is None:
            # Without an API key: the core's answer, the browser's.
            core.pdf_report_answer(project_id, run_id, body, *listing)
        if not settings.model_for("review").startswith("claude-opus-"):
            raise StorageError("pdf_opus_required: PDF分析にはOpusを設定してください。")
        with self._lock:
            return self._export(project_id, run_id, plan, link, settings)

    def _export(self, project_id: str, run_id: str, plan: Any, link: Any, settings: Any) -> bytes:
        from sheltersim.llm.link import ask
        from sheltersim.web.projects import answers

        core, files, store, api = answers(), self.api.files, self.api.store, self.api
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
            if saved.get("identity") == identity:
                try:
                    pdf = base64.b64decode(saved["pdf_base64"], validate=True)
                    if pdf.startswith(b"%PDF-") and digest(saved["pdf_base64"]) == saved.get(
                        "pdf_hash"
                    ):
                        return pdf
                except (ValueError, KeyError):
                    pass  # Typeset a damaged PDF again, from the kept analysis's tape.
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
                if plan.minute not in saved:
                    raise StorageError(
                        "pdf_saved_point_required: このRunは別のモデルの版で計算したため、"
                        "保存した時点だけを表示します。レポートは保存した時点"
                        f"（{saved[-1] if saved else 0}分など）を選んでください。"
                    )
                exact = True
            branch_run = api.live.at(project_id, run_id, record, plan.minute, exact=exact)
            frame = branch_run.frame(run_id, branch_id, record["computed_until"])
            table = core.KpiTable()
            for kind, raw in files.reader(project_id, run_id, branch_id).sample_pieces():
                if kind == "sample_tables":
                    table.read_sample_tables(raw)
                else:
                    table.read_sample_block(raw)
            plan.add_branch(branch_run, frame, table, journal)
        attempt_path = store._path(project_id, "runs", run_id, "pdf-attempts", f"{key}.json")
        attempt = store._read(attempt_path)["next_attempt"] if attempt_path.exists() else 0
        analysis = plan.analysis(scenario, attempt)
        reply = ask(link, analysis.task())
        try:
            checked = json.loads(analysis.read(reply))
        except core.Refused:
            record = json.loads(reply) if reply is not None else {}
            if record.get("api_request_count") or record.get("cache_hit"):
                # A retry after an answer that was paid for and refused is a new request; the
                # receipts of the earlier one stay.
                with store.transaction():
                    store._write(attempt_path, {"schema_version": 1, "next_attempt": attempt + 1})
            raise
        data, facts = json.loads(analysis.data()), json.loads(analysis.facts())
        from sheltersim.web.pdf_render import render_pdf

        try:
            pdf = render_pdf(data, checked, facts)
        except Exception as exc:
            raise StorageError(
                "pdf_render_failed: PDFの組版に失敗しました。再試行してください。"
            ) from exc
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
        encoded = base64.b64encode(pdf).decode("ascii")
        artifact = {
            "schema_version": 1,
            "identity": identity,
            "data": data,
            "facts": facts,
            "analysis": checked,
            "pdf_hash": digest(encoded),
            "pdf_base64": encoded,
        }
        with store.transaction():
            store._write(path, safe_document(artifact))
        return pdf
