"""CLI entry points for durable projects; simulation work remains in shared services."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from sheltersim.experiments.scenario import ScenarioSpec
from sheltersim.storage.artifacts import ArtifactStore
from sheltersim.storage.projects import ProjectStore, StorageError


def configure_parser(subparsers: Any) -> None:
    parser = subparsers.add_parser(
        "project", help="save, reopen, resume and inspect durable projects (the screen's routes)"
    )
    parser.add_argument("--root", type=Path, default=Path("runs/.projects"))
    actions = parser.add_subparsers(dest="project_action", required=True)
    actions.add_parser("list")
    for action in (
        "create",
        "show",
        "save-scenario",
        "new-run",
        "resume",
        "frame",
        "delete",
        "prepare",
        "approve-draft",
        "usage",
        "draft-save",
        "draft-show",
        "history",
    ):
        command = actions.add_parser(action)
        command.add_argument("--project-id", required=True)
        if action in {
            "new-run",
            "resume",
            "frame",
            "delete",
            "approve-draft",
            "usage",
            "history",
        }:
            command.add_argument("--run-id", required=True)
        if action == "prepare":
            command.add_argument("--text", required=True)
            command.add_argument("--input-json", type=Path)
            command.add_argument("--draft-id")
        if action in {"draft-save", "draft-show"}:
            command.add_argument("--draft-id", required=True)
        if action in {"draft-save", "history"}:
            command.add_argument("--input-json", type=Path, required=True)
        if action == "history":
            command.add_argument("--branch-id", default="main")
        if action == "draft-save":
            command.add_argument("--expected-hash")
        if action == "approve-draft":
            command.add_argument("--preview-id", required=True)
            command.add_argument("--approved-hash", required=True)
        if action == "save-scenario":
            command.add_argument("--scenario", type=Path, required=True)
            command.add_argument(
                "--approved-hash", required=True, help="SHA256 of reviewed canonical ScenarioSpec"
            )
        if action == "new-run":
            command.add_argument("--scenario-id", required=True)
            command.add_argument("--scenario-version", type=int, required=True)
        if action in {"resume", "frame"}:
            command.add_argument("--branch-id", default="main")
            command.add_argument("--minute", type=int, required=True)
        if action == "delete":
            command.add_argument(
                "--confirmation", required=True, help="repeat exact inactive Run ID"
            )


def execute(arguments: Any) -> dict[str, Any]:
    """A Project command, through the routes the screen calls (web/projects.py, #427). A
    refusal of the core's answers is raised as the server's StorageError (its text and code)."""
    from sheltersim.web.projects import answers

    try:
        return _execute(arguments)
    except answers().Refused as refusal:
        status, error, code, _ = refusal.args
        if status == 400:
            raise ValueError(error) from None
        raise StorageError(error, code=code) from None


def _execute(arguments: Any) -> dict[str, Any]:
    from urllib.parse import urlencode

    from sheltersim.web.projects import ProjectAPI

    store = ProjectStore(arguments.root)
    action = arguments.project_action
    api = ProjectAPI(store, recover=False)
    try:
        if action == "list":
            return api.get([])
        project_id = arguments.project_id
        if action == "create":
            return api.post([], {"project_id": project_id})
        if action == "show":
            return api.get([project_id])
        if action == "prepare":
            body = (
                json.loads(arguments.input_json.read_text(encoding="utf-8"))
                if arguments.input_json
                else {}
            )
            body["text"] = arguments.text
            if arguments.draft_id:
                body["draft_id"] = arguments.draft_id
            return api.post([project_id, "scenario-preview"], body)
        if action == "approve-draft":
            return api.post(
                [project_id, "scenario-approve"],
                {
                    "run_id": arguments.run_id,
                    "preview_id": arguments.preview_id,
                    "approved_hash": arguments.approved_hash,
                    "approved": True,
                },
            )
        if action == "save-scenario":
            from sheltersim.web.stored_runs import save_scenario

            scenario = ScenarioSpec.load(arguments.scenario)
            return save_scenario(api.files, project_id, scenario, arguments.approved_hash)
        if action == "new-run":
            return api.post(
                [project_id, "runs"],
                {
                    "run_id": arguments.run_id,
                    "scenario_id": arguments.scenario_id,
                    "scenario_version": arguments.scenario_version,
                },
            )
        if action == "usage":
            return api.get([project_id, "runs", arguments.run_id, "usage"])
        if action == "draft-show":
            return ArtifactStore(store).draft(project_id, arguments.draft_id)
        if action == "draft-save":
            from sheltersim.contracts.workspace import DraftInput

            return ArtifactStore(store).save_draft(
                project_id,
                arguments.draft_id,
                DraftInput(
                    content=json.loads(arguments.input_json.read_text(encoding="utf-8")),
                    expected_hash=arguments.expected_hash,
                ),
            )
        run = [project_id, "runs", arguments.run_id]
        if action == "history":
            query = json.loads(arguments.input_json.read_text(encoding="utf-8"))
            return api.get([*run, "branches", arguments.branch_id, "history"], urlencode(query))
        if action == "delete":
            return api.post([*run, "delete"], {"confirmation": arguments.confirmation})
        if action == "frame":
            return api.get(
                [*run, "branches", arguments.branch_id, "timeline"], f"minute={arguments.minute}"
            )
        if action == "resume":
            api.post(
                [*run, "resume"],
                {"branch_id": arguments.branch_id, "target_minute": arguments.minute},
            )
            api.wait_idle()
            branch = next(
                each
                for each in api.get(run)["branches"]
                if each["branch_id"] == arguments.branch_id
            )
            if branch["status"] == "failed":
                raise StorageError("the computation failed")
            return {
                "run_id": arguments.run_id,
                "branch_id": arguments.branch_id,
                "minute": branch["computed_until"],
                "status": branch["status"],
            }
    finally:
        api.close()
    raise StorageError("unknown project command")
