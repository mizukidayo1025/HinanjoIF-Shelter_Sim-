"""The Scenario screen's routes (a Scenario made from text and approved, #301) and the routes
of an intervention and a question, whose answers are the core's (#449, #427)."""

from __future__ import annotations

import hashlib
import json
import threading
import uuid
from collections import OrderedDict
from typing import Any

from sheltersim.contracts.workspace import DraftInput
from sheltersim.experiments.initial_scenario import InitialScenarioBuilder, InitialScenarioRequest
from sheltersim.llm.accounting import StoredResponseTape, accounted_router
from sheltersim.llm.region_prepare import parse_scenario_text
from sheltersim.region.models import RegionResearchRequest
from sheltersim.region.population import (
    age_bands_from_observations,
    household_mix_from_observations,
)
from sheltersim.region.research import RegionResearchService
from sheltersim.storage.artifacts import ArtifactStore
from sheltersim.storage.projects import StorageError, digest


class ProjectWorkspace:
    def __init__(self, api: Any) -> None:
        self.api, self.store = api, api.store
        self.research = RegionResearchService()
        self.drafts: OrderedDict[str, dict[str, Any]] = OrderedDict()
        self.artifacts = ArtifactStore(self.store)
        self._lock = threading.RLock()

    def _cache(self, value: dict[str, Any]) -> str:
        key = uuid.uuid4().hex
        content = {
            k: v.model_dump(mode="json") if hasattr(v, "model_dump") else v
            for k, v in value.items()
        }
        self.artifacts.save_draft(
            value["project_id"], key, DraftInput(kind=value["kind"], content=content)
        )
        self.drafts[key] = value
        while len(self.drafts) > 20:
            self.drafts.popitem(last=False)
        return key

    def _preview(self, project_id: str, preview_id: str) -> dict[str, Any]:
        content = self.artifacts.draft(project_id, preview_id)["content"]
        if content["project_id"] != project_id:
            raise StorageError("draft belongs to another Project")
        if content["kind"] == "scenario":
            from sheltersim.experiments.scenario import ScenarioSpec

            content["scenario"] = ScenarioSpec.model_validate(content["scenario"])
        return content

    def _preparation_account(self, project_id: str, requested: str | None = None) -> str:
        if requested is not None:
            self.store._path(project_id, "usage", requested)
            return requested
        # Old clients need not send draft_id: retries/edits still share one allowance.
        with self.store.transaction():
            path = self.store._path(project_id, "preparation.json")
            pending = self.store._read(path) if path.exists() else None
            if pending and pending["status"] == "pending":
                return pending["account_id"]
            account_id = uuid.uuid4().hex
            self.store._write(
                path, {"schema_version": 1, "account_id": account_id, "status": "pending"}
            )
            return account_id

    def _bind_preparation(self, project_id: str, account_id: str, run_id: str) -> None:
        with self.store.transaction():
            path = self.store._path(project_id, "preparation.json")
            if path.exists():
                pending = self.store._read(path)
                if pending["account_id"] == account_id:
                    self.store._write(path, {**pending, "status": "bound", "run_id": run_id})

    def preview(self, project_id: str, body: dict[str, Any]) -> dict[str, Any]:
        from sheltersim.web.projects import fields

        fields(
            body,
            {"text"},
            {
                "request",
                "estat_stats_data_id",
                "estat_area_code",
                "estat_period_code",
                "estat_category_codes",
                "selected_shelter_id",
                "draft_id",
                "target_date",
                "weather_area_code",
            },
        )
        self.api.files.project(project_id)
        draft_id = self._preparation_account(project_id, body.get("draft_id"))
        tape = StoredResponseTape(
            self.store, self.store._path(project_id, "usage-tapes", f"{draft_id}.json")
        )
        router = accounted_router(
            self.store, project_id, account_id=draft_id, operation="scenario_prepare", tape=tape
        )
        text = body["text"]
        if not isinstance(text, str):
            raise StorageError("scenario text must be a string")
        if text.strip():
            parsed = parse_scenario_text(text, router=router)
            request = (
                InitialScenarioRequest.model_validate(body["request"])
                if body.get("request")
                else parsed.request
            )
            input_sha256 = parsed.input_sha256
            extraction_source = parsed.source
            parse_warnings = list(parsed.warnings)
            missing_fields = list(parsed.missing_fields)
        elif body.get("request"):
            request = InitialScenarioRequest.model_validate(body["request"])
            structured_input = json.dumps(
                request.model_dump(mode="json"),
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
            input_sha256 = hashlib.sha256(structured_input).hexdigest()
            extraction_source = "structured_form"
            parse_warnings = []
            missing_fields = []
        else:
            return {
                "can_start": False,
                "missing_fields": ["municipality", "disaster", "evacuees"],
                "warnings": [],
                "draft_id": draft_id,
            }
        if request is None:
            return {
                "can_start": False,
                "missing_fields": missing_fields,
                "warnings": parse_warnings,
                "draft_id": draft_id,
            }
        research = self.research.research(
            RegionResearchRequest(
                municipality_query=request.municipality,
                municipality_code=request.municipality_code,
                disaster=request.disaster,
                season=request.season,
                selected_municipality_name=request.municipality,
                estat_stats_data_id=body.get("estat_stats_data_id"),
                estat_area_code=body.get("estat_area_code"),
                estat_period_code=body.get("estat_period_code"),
                estat_category_codes=body.get("estat_category_codes", {}),
                target_date=body.get("target_date", request.target_date),
                weather_area_code=body.get("weather_area_code", request.weather_area_code),
            ),
            router=router,
        )
        draft = InitialScenarioBuilder().build(
            request,
            research=research,
            selected_shelter_id=body.get("selected_shelter_id"),
            age_bands=age_bands_from_observations(research.population_observations),
            household_mix=household_mix_from_observations(research.population_observations),
        )
        document = draft.scenario.model_dump(mode="json")
        # A public building's address is not needed in anonymous stored experiments.
        document["provenance"].get("selected_shelter", {}).pop("address", None)
        for shelter in document["provenance"].get("region_research", {}).get("shelters", []):
            shelter.pop("address", None)
        document["provenance"]["scenario_input"] = {
            "sha256": input_sha256,
            "extraction_source": extraction_source,
        }
        from sheltersim.experiments.scenario import ScenarioSpec

        scenario = ScenarioSpec.model_validate(document)
        approval_hash = digest(json.loads(scenario.canonical_json()))
        result = {
            "scenario": json.loads(scenario.canonical_json()),
            "approved_hash": approval_hash,
            "can_start": draft.can_start,
            "approval_blockers": list(draft.approval_blockers),
            "warnings": [*parse_warnings, *draft.warnings],
            "research": research.model_dump(mode="json"),
            "request": request.model_dump(mode="json"),
            "llm_usage": {
                "mode": router.mode.value,
                "api_request_count": router.api_request_count,
                "spent_usd": router.spent_usd,
            },
        }
        result["draft_id"] = draft_id
        research_record = research.model_dump(mode="json")
        for shelter in research_record.get("shelters", []):
            shelter.pop("address", None)
        design_record = (
            draft.facility_design.model_dump(mode="json") if draft.facility_design else None
        )
        result["facility_design"] = design_record
        result["geometry_hash"] = digest(design_record["geometry"]) if design_record else None
        result["preview_id"] = self._cache(
            {
                "usage_account_id": draft_id,
                "kind": "scenario",
                "project_id": project_id,
                "scenario": scenario,
                "can_start": draft.can_start,
                "hash": approval_hash,
                "approval_blockers": list(draft.approval_blockers),
                "facility_design": design_record,
                "request": request.model_dump(mode="json"),
                "research": research_record,
            }
        )
        return result

    def approve(self, project_id: str, body: dict[str, Any]) -> bytes:
        """`POST {project}/scenario-approve`: the reviewed Scenario saved as a new revision and a
        Run of it created (`POST {project}/runs`)."""
        from sheltersim.web.projects import fields
        from sheltersim.web.stored_runs import save_scenario

        fields(body, {"preview_id", "approved_hash", "approved", "run_id"})
        preview = self._preview(project_id, body["preview_id"])
        if not preview or preview["kind"] != "scenario" or preview["project_id"] != project_id:
            raise StorageError("preview expired; generate and review a new preview")
        if (
            body["approved"] is not True
            or body["approved_hash"] != preview["hash"]
            or not preview["can_start"]
        ):
            raise StorageError("approval must match a startable reviewed scenario")
        existing = self.api.files.run_file(project_id, body["run_id"], "run.json")
        if existing is not None:
            existing = json.loads(existing)
            if existing["scenario_hash"] != preview["hash"]:
                raise StorageError("run_id_conflict")
            scenario_id, version = existing["scenario_id"], existing["scenario_version"]
        else:
            saved = save_scenario(self.api.files, project_id, preview["scenario"], preview["hash"])
            scenario_id, version = saved["scenario_id"], saved["scenario_version"]
        result = self.api.create_run(
            project_id,
            {
                "run_id": body["run_id"],
                "scenario_id": scenario_id,
                "scenario_version": version,
                "usage_account_id": preview["usage_account_id"],
            },
        )
        self._bind_preparation(project_id, preview["usage_account_id"], body["run_id"])
        self.drafts.pop(body["preview_id"], None)
        return result

    # ---- Interventions and questions (the core's answers, #449) -------------------------------

    def intervention_preview(
        self, project_id: str, run_id: str, branch_id: str, body: bytes
    ) -> bytes:
        """`POST …/intervention-preview` (`{minute, program}` or `{minute, text}`): the branch's
        Run at the minute, the typed program the screen built from the core's catalog (#584) or
        the text read (by the offline compiler, or with an API key by the configured router
        through the core, #463), the program's checks on the world, and the draft kept for the
        approval (the browser's `interventionPreview`)."""
        from sheltersim.web.projects import answers

        core, files = answers(), self.api.files
        plan = core.preview_plan(
            body, project_id, run_id, *files.run_listing(project_id, run_id), branch_id
        )
        self._require_current_model(project_id, run_id, branch_id)
        record = json.loads(plan.record_json())
        run = self.api.live.at(project_id, run_id, record, plan.minute)
        preview_id = uuid.uuid4().hex
        answer, draft = self._preview_answer(project_id, run_id, branch_id, plan, run, preview_id)
        if draft is not None:
            with files.transaction():
                files.keep_draft(project_id, preview_id, draft)
        return answer

    def _preview_answer(
        self, project_id: str, run_id: str, branch_id: str, plan: Any, run: Any, preview_id: str
    ) -> tuple[bytes, bytes | None]:
        """The preview's answer and draft: without an API key the offline compiler's; with one,
        the core asks the configured router to read the text (one repair at most) and checks the
        plan, as `compile_natural_language` did (#463)."""
        from sheltersim.llm.config import LLMErrorPolicy
        from sheltersim.llm.link import COMPILER_TAPE, analysis_link, ask

        link = analysis_link(
            self.store,
            project_id,
            run_id,
            operation="compiler",
            branch_id=branch_id,
            tape=COMPILER_TAPE,
        )
        compiling = (
            None
            if link is None
            else plan.compiling(run, link.router.settings.error_policy is LLMErrorPolicy.STOP)
        )
        if compiling is None:
            return plan.answer(run, preview_id)
        step, data = compiling.read(ask(link, compiling.task()))
        while step == "ask":
            step, data = compiling.read(ask(link, data))
        return plan.answer_compiled(run, preview_id, data)

    def intervention_apply(
        self, project_id: str, run_id: str, branch_id: str, body: bytes
    ) -> bytes:
        """`POST …/intervention-apply`: the draft and the approval checked, then the child made
        at once (`ProjectAPI.fork`); its outcome is the Run's `last_fork`."""
        from sheltersim.web.projects import answers

        core, files, api = answers(), self.api.files, self.api
        draft = files.draft(project_id, core.apply_draft_id(body))
        plan = core.apply_plan(
            body, draft, project_id, run_id, branch_id, api.busy(project_id, run_id, branch_id)
        )
        # A Run of another model version is view-only: no fork, no EffectProgram.
        self._require_current_model(project_id, run_id, branch_id)
        record = json.loads(plan.fork_checks(*files.run_listing(project_id, run_id)))
        api.keep_fork_outcome(project_id, run_id, None)
        try:
            child, _, state_hash = api.fork(
                project_id,
                run_id,
                record["branch_id"],
                plan.child_id,
                plan.minute,
                plan.program_text,
                plan.hash,
                _draft_state_hash(draft),
            )
            outcome = core.created_outcome(child["branch_id"], plan.minute, state_hash)
        except StorageError as exc:
            outcome = core.failed_outcome(str(exc), True, exc.code)
        except ValueError as exc:
            outcome = core.failed_outcome(str(exc), True, None)
        except Exception as exc:
            outcome = core.failed_outcome(str(exc), False, None)
        api.keep_fork_outcome(project_id, run_id, outcome)
        return plan.answer()

    def question(self, project_id: str, run_id: str, branch_id: str, body: bytes) -> bytes:
        """`POST …/questions`: the request's and the evidence's checks, the answer, and the turn
        kept in the branch's stored Run (the browser's `questionResponse`). Without an API key the
        answer is the one without a Provider; with one, the core builds the evidence packet and
        the task, the configured router answers it, and the core checks the answer (#463)."""
        import secrets
        import time

        from sheltersim.web.projects import answers

        core, files, api = answers(), self.api.files, self.api
        plan = core.question_plan(body, project_id, run_id, branch_id)
        listing = files.run_listing(project_id, run_id)
        plan.check_branch(*listing)
        plan.evidence(files.run_scenario_bytes(project_id, run_id))
        minute = plan.minute
        record = json.loads(core.branch_of(project_id, run_id, *listing, branch_id))
        compared = plan.check_frame(api.frame(project_id, run_id, record, minute))
        if compared:
            core.comparison_range(compared, minute, project_id, run_id, *listing)
            core.compare_plan(
                f"branch_ids={','.join(compared)}&minute={minute}", project_id, run_id, *listing
            ).receipts(api.journals(project_id, run_id, compared))
        random, turn_id = secrets.token_hex(12), f"t-{time.time_ns()}-{secrets.token_hex(4)}"
        link = self.analysis_link(project_id, run_id, "questions", branch_id)
        if link is None:
            answer, turn = plan.answer(random, turn_id)
        else:
            from sheltersim.llm.link import ask

            comparison = None
            if compared:
                comparison_plan = core.compare_plan(
                    f"branch_ids={','.join(compared)}&minute={minute}",
                    project_id,
                    run_id,
                    *listing,
                )
                comparison_plan.receipts(api.journals(project_id, run_id, compared))
                records = json.loads(comparison_plan.records_json())
                comparison = comparison_plan.answer(
                    run_id, [api.frame(project_id, run_id, item, minute) for item in records]
                )
            asking = plan.ask(
                api.frame(project_id, run_id, record, minute),
                comparison,
                files.run_file(project_id, run_id, "evidence.json"),
                files.run_scenario_bytes(project_id, run_id),
                files.conversation_pieces(project_id, run_id, branch_id),
                random,
            )
            reply = ask(link, asking.task())
            answer, turn = asking.answer(reply, link.router.mode.value, turn_id)
        with files.claim_branch(project_id, run_id, branch_id) as lease, files.transaction():
            files.keep_turn(lease, project_id, run_id, branch_id, turn)
        return answer

    def analysis_link(self, project_id: str, run_id: str, operation: str, branch_id: str) -> Any:
        """The link a question asks through (`sheltersim.llm.link.analysis_link`), or None
        without an API key."""
        from sheltersim.llm.link import analysis_link

        return analysis_link(
            self.store, project_id, run_id, operation=operation, branch_id=branch_id
        )

    def _require_current_model(self, project_id: str, run_id: str, branch_id: str) -> None:
        from sheltersim.web.projects import answers

        files = self.api.files
        answers().require_current_model(
            files.branch_file(project_id, run_id, branch_id, "journal.jsonl") or b"",
            files.run_scenario_bytes(project_id, run_id),
        )


def _draft_state_hash(draft: bytes) -> str:
    """The digest of the parent's world the preview saw, which its draft keeps: the fork is
    refused where the parent's world at the minute is another ("source state changed")."""
    document = json.loads(draft)
    return document.get("content", document)["state_hash"]
