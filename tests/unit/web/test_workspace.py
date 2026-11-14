"""The Project screen's draft routes: the structured-form preview, its saved replies and the
regional research it uses.

Every draft comes from the same Scenario preview the screen uses (tests/support/generated.py
ui_project_api). The household correction route and the imported plans went with the older
screens (#376).
"""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from sheltersim.core.world import WorldState
from sheltersim.experiments.branch import BranchRuntime
from sheltersim.experiments.initial_scenario import InitialScenarioRequest
from sheltersim.experiments.timeline import TimelineStore
from sheltersim.llm.config import LLMMode, LLMSettings
from sheltersim.llm.contracts import ProviderResponse
from sheltersim.storage.projects import ProjectStore, digest
from sheltersim.web.projects import ProjectAPI
from sheltersim.web.workspace import ProjectWorkspace


def _preview(api, request: dict) -> dict:
    return api.workspace.post(["project", "scenario-preview"], {"text": "", "request": request})


def test_preparation_edits_share_account_even_without_new_client_field(
    project_store_at, tmp_path
) -> None:
    store = project_store_at(tmp_path)
    workspace = SimpleNamespace(store=store)
    first = ProjectWorkspace._preparation_account(workspace, "project")
    assert ProjectWorkspace._preparation_account(SimpleNamespace(store=store), "project") == first
    ProjectWorkspace._bind_preparation(workspace, "project", first, "approved-run")
    assert ProjectWorkspace._preparation_account(workspace, "project") != first


def test_project_preview_accepts_structured_form_without_free_text(ui_project_api) -> None:
    request = InitialScenarioRequest(
        municipality="Synthetic municipality",
        disaster="earthquake",
        evacuees=8,
        duration_days=1,
        seed=1,
        model_parameter_overrides={
            "body_dynamics.sleep_debt_stress_rate_by_agent.*": {"A001": 0.003},
            "facility_queues.abandon_after_minutes_by_service.*": {"toilet": 60},
            "zone_environment.baseline_by_zone.*": {
                "sleeping": {"privacy_level": 0.2, "temperature_c": 30.0}
            },
        },
        evacuation_selection={"age_band_multipliers": {"child": 0, "adult": 1, "older_adult": 0}},
        field_origins={
            "municipality": "user",
            "disaster": "user",
            "evacuees": "user",
            "duration_days": "user",
        },
    )

    preview = _preview(ui_project_api, request.model_dump(mode="json"))

    scenario = preview["scenario"]
    assert scenario["model_configs"]["facility_queues"]["abandon_after_minutes_by_service"] == {
        "toilet": 60
    }
    assert scenario["model_configs"]["body_dynamics"]["sleep_debt_stress_rate_by_agent"] == {
        "A001": 0.003
    }
    assert scenario["model_configs"]["zone_environment"]["baseline_by_zone"]["sleeping"] == {
        "ventilation_effectiveness": 0.7,
        "lighting_level": 0.5,
        "privacy_level": 0.2,
        "temperature_c": 30.0,
    }
    overrides = preview["request"]["model_parameter_overrides"]
    assert overrides["facility_queues.abandon_after_minutes_by_service.*"] == {"toilet": 60}
    assert overrides["body_dynamics.sleep_debt_stress_rate_by_agent.*"] == {"A001": 0.003}
    assert overrides["zone_environment.baseline_by_zone.*"] == {
        "sleeping": {"privacy_level": 0.2, "temperature_c": 30.0}
    }
    provenance = scenario["provenance"]["scenario_input"]
    normalized_request = InitialScenarioRequest.model_validate(preview["request"])
    expected_source = digest(normalized_request.model_dump(mode="json"))
    assert preview["can_start"] is True
    assert provenance["extraction_source"] == "structured_form"
    assert provenance["sha256"] == expected_source
    assert preview["llm_usage"]["api_request_count"] == 0


def test_repeated_identical_preview_reuses_the_saved_reply(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from datetime import UTC, datetime

    from sheltersim.region.models import RegionResearchResult
    from sheltersim.web import workspace as workspace_module

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    calls = []

    class ContractProvider:
        """Return the task's own contract-shaped answer, as an accepted reply."""

        def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
            del max_output_tokens, timeout_seconds
            calls.append(task.prompt_version)
            return ProviderResponse(
                text=task.fallback_text,
                model_id=model_id,
                input_tokens=10,
                output_tokens=5,
                elapsed_ms=0.0,
            )

    real_router = workspace_module.accounted_router

    def live_like_router(*args, **kwargs):
        settings = LLMSettings(mode=LLMMode.RECORD, api_key="mock-key", budget_usd=10.0)
        router = real_router(*args, **{**kwargs, "settings": settings})
        router.provider.provider = ContractProvider()  # keep the real usage ledger
        return router

    monkeypatch.setattr(workspace_module, "accounted_router", live_like_router)
    store = ProjectStore(tmp_path / "projects")
    store.create_project("project")
    api = ProjectAPI(store, recover=False)
    monkeypatch.setattr(
        api.workspace.research,
        "research",
        lambda query, **_kwargs: RegionResearchResult(
            request=query, researched_at=datetime.now(UTC)
        ),
    )
    body = {"text": "横浜市で地震が発生。避難者30人、期間は1日。"}
    try:
        first = api.workspace.preview("project", body)
        second = api.workspace.preview("project", body)
    finally:
        api.close()

    source = "extraction_source"
    assert first["scenario"]["provenance"]["scenario_input"][source] == "prepare_llm"
    # Before stored tapes, the ledger refused this identical paid request.
    assert second["scenario"]["provenance"]["scenario_input"][source] == "prepare_llm"
    assert second["llm_usage"]["api_request_count"] == 0
    assert calls == ["region-scenario-extraction-v1"]


def test_repeated_identical_intervention_preview_reuses_the_saved_reply(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, small_benchmark_scenario
) -> None:
    from sheltersim.web import workspace as workspace_module

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    calls = []

    class AcceptedProvider:
        def complete(self, *, model_id, task, max_output_tokens, timeout_seconds):
            del max_output_tokens, timeout_seconds
            calls.append(task.prompt_version)
            return ProviderResponse(
                text=task.fallback_text,
                model_id=model_id,
                input_tokens=10,
                output_tokens=5,
                elapsed_ms=0.0,
            )

    real_router = workspace_module.accounted_router

    def live_like_router(*args, **kwargs):
        settings = LLMSettings(mode=LLMMode.RECORD, api_key="mock-key", budget_usd=10.0)
        router = real_router(*args, **{**kwargs, "settings": settings})
        router.provider.provider = AcceptedProvider()  # keep the real usage ledger
        return router

    monkeypatch.setattr(workspace_module, "accounted_router", live_like_router)
    scenario = small_benchmark_scenario(30)
    store = ProjectStore(tmp_path / "projects")
    store.create_project("project")
    saved = store.save_scenario(
        "project", scenario, approved_hash=digest(json.loads(scenario.canonical_json()))
    )
    store.create_run("project", "run", scenario.id, saved["scenario_version"])
    TimelineStore(store, "project", "run").record(
        BranchRuntime.from_world(WorldState.from_scenario(scenario))
    )
    api = ProjectAPI(store, recover=False)
    body = {"minute": 0, "text": "information_desk ゾーンにスタッフを2人追加してください。"}
    try:
        first = api.workspace.intervention("project", "run", "main", dict(body))
        calls_after_first = len(calls)
        second = api.workspace.intervention("project", "run", "main", dict(body))
    finally:
        api.close()

    assert calls_after_first >= 1 and first["fallback_used"] is False
    # Before stored compiler tapes, the ledger refused this identical paid request.
    assert second["fallback_used"] is False
    assert len(calls) == calls_after_first


def _researched_preview(api, generated_request, **selections) -> dict:
    """A Project screen preview of 合成県合成市, researched over the fake public services."""
    from support.public_data import MUNICIPALITY

    fields = {key: selections.pop(key) for key in list(selections) if key in REQUEST_FIELDS}
    request = generated_request(municipality=MUNICIPALITY, **fields)
    return api.workspace.preview("project", {"text": "", "request": request, **selections})


# Selections the Project screen puts in the structured form; the others go beside it.
REQUEST_FIELDS = {"municipality_code", "target_date", "weather_area_code"}


def test_a_preview_uses_the_selected_statistics_and_shelter(
    researching_project_api, generated_request
) -> None:
    from support.public_data import (
        ESTAT_PERIOD_CODE,
        ESTAT_TABLE_ID,
        MUNICIPALITY_CODE,
        SCHOOL_SHELTER_ID,
    )

    first = _researched_preview(researching_project_api, generated_request)
    # Before any selection, the screen offers the shelters and tables to choose from.
    assert [item["id"] for item in first["research"]["shelters"]][0] == SCHOOL_SHELTER_ID
    assert [item["table_id"] for item in first["research"]["statistical_tables"]] == [
        ESTAT_TABLE_ID
    ]
    assert not first["research"]["population_observations"]

    preview = _researched_preview(
        researching_project_api,
        generated_request,
        municipality_code=MUNICIPALITY_CODE,
        estat_stats_data_id=ESTAT_TABLE_ID,
        estat_area_code=MUNICIPALITY_CODE,
        estat_period_code=ESTAT_PERIOD_CODE,
        selected_shelter_id=SCHOOL_SHELTER_ID,
    )

    assert preview["can_start"], preview["approval_blockers"]
    provenance = preview["scenario"]["provenance"]
    generation = provenance["population_generation"]
    # The selected table counts only people aged 65 and over.
    assert generation["evacuee_age_distribution"] == {
        "0～14歳": 0.0,
        "15～64歳": 0.0,
        "65歳以上": 1.0,
    }
    assert {source for band in generation["age_bands"] for source in band["source_ids"]} == {
        "SRC-ESTAT-API"
    }
    assert provenance["selected_shelter"]["name"] == "合成小学校"
    assert provenance["selected_shelter"]["source_id"] in {
        item["id"] for item in provenance["sources"]
    }


def test_a_partial_municipality_name_lists_codes_until_one_is_chosen(
    researching_project_api, generated_request
) -> None:
    api = researching_project_api
    request = generated_request(municipality="合成")

    listed = api.workspace.preview("project", {"text": "", "request": request})
    chosen = api.workspace.preview(
        "project", {"text": "", "request": {**request, "municipality_code": "99901"}}
    )

    register = listed["research"]["shelter_register"]
    assert register["status"] == "unresolved_municipality"
    assert [item["municipality_code"] for item in register["municipality_candidates"]] == [
        "99901",
        "99902",
    ]
    assert not listed["research"]["shelters"]
    assert chosen["research"]["shelter_register"]["status"] == "matched"
    assert len(chosen["research"]["shelters"]) == 2


def test_a_preview_survives_a_public_service_dropping_the_connection(
    researching_project_api, generated_request, public_data
) -> None:
    from support.public_data import ESTAT_PERIOD_CODE, ESTAT_TABLE_ID, MUNICIPALITY_CODE

    # Before #208, the dropped connection failed the whole preview with HTTP 500.
    public_data.fail("api.e-stat.go.jp", "disconnected")

    preview = _researched_preview(
        researching_project_api,
        generated_request,
        municipality_code=MUNICIPALITY_CODE,
        estat_stats_data_id=ESTAT_TABLE_ID,
        estat_area_code=MUNICIPALITY_CODE,
        estat_period_code=ESTAT_PERIOD_CODE,
    )

    assert preview["can_start"], preview["approval_blockers"]
    assert "e-Stat APIから統計を取得できませんでした。" in preview["research"]["unknowns"]
    assert len(preview["research"]["shelters"]) == 2


def _selected_statistics_preview(api, generated_request) -> dict:
    from support.public_data import ESTAT_PERIOD_CODE, ESTAT_TABLE_ID, MUNICIPALITY_CODE

    return _researched_preview(
        api,
        generated_request,
        municipality_code=MUNICIPALITY_CODE,
        estat_stats_data_id=ESTAT_TABLE_ID,
        estat_area_code=MUNICIPALITY_CODE,
        estat_period_code=ESTAT_PERIOD_CODE,
    )


def test_selected_statistics_with_a_total_row_are_used_without_it(
    researching_project_api, generated_request, public_data
) -> None:
    # Most e-Stat population tables count everyone in a 総数 class of the age axis.
    # Before #209 the statistics were then dropped for assumed ages, and nobody was told.
    public_data.age_classes.insert(0, ("000", "総数", "5000"))

    preview = _selected_statistics_preview(researching_project_api, generated_request)

    assert len(preview["research"]["population_observations"]) == 4
    generation = preview["scenario"]["provenance"]["population_generation"]
    assert generation["evacuee_age_distribution"]["65歳以上"] == 1.0
    assert "統計の合計の行（総数）を除いて年齢構成に使いました。" in preview["warnings"]


def test_selected_statistics_with_subtotals_are_unused_and_reported(
    researching_project_api, generated_request, public_data
) -> None:
    # Five-year bands beside their subtotal 15歳未満 overlap. Before #209 they reached
    # the population generator, which refused them and failed the whole preview.
    public_data.age_classes[:] = [
        ("101", "0～4歳", "10"),
        ("102", "5～9歳", "10"),
        ("103", "10～14歳", "10"),
        ("150", "15歳未満", "30"),
        ("200", "15～64歳", "50"),
        ("300", "65歳以上", "20"),
    ]

    preview = _selected_statistics_preview(researching_project_api, generated_request)

    assert preview["can_start"], preview["approval_blockers"]
    generation = preview["scenario"]["provenance"]["population_generation"]
    assert {source for band in generation["age_bands"] for source in band["source_ids"]} == {
        "ASM-SYNTHETIC-AGE-BANDS-V1"
    }
    assert any(
        warning.startswith("取得した統計（6行）は、年齢構成にも世帯人数にも使えません")
        for warning in preview["warnings"]
    )
