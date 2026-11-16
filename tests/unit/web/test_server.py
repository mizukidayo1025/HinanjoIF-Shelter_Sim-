"""serve_inspector gives its projects directory the saved-data limit it is started with;
llm_status says whether the server's AI features are ON (#581).

Page routing over HTTP is checked in tests/integration/test_web_routes.py.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from sheltersim.storage.projects import ProjectStore
from sheltersim.web import server


def test_serve_inspector_limits_the_projects_directory_to_its_storage_option(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    stores: list[ProjectStore] = []
    built = server.ProjectAPI

    def project_api(store: ProjectStore, **options: Any) -> Any:
        stores.append(store)
        return built(store, **options)

    class StoppingServer:
        server_port = 0

        def __init__(self, *_: Any) -> None:
            pass

        def serve_forever(self) -> None:
            raise KeyboardInterrupt

        def server_close(self) -> None:
            pass

    monkeypatch.setattr(server, "ProjectAPI", project_api)
    monkeypatch.setattr(server, "HTTPServer", StoppingServer)
    with pytest.raises(KeyboardInterrupt):
        server.serve_inspector(tmp_path, project_storage_bytes=14_000_000_000)
    # The computing and indexing processes take their limit from this store.
    assert [store.max_bytes for store in stores] == [14_000_000_000]
    assert stores[0].usage()["limit_bytes"] == 14_000_000_000


@pytest.mark.parametrize(
    ("mode", "api_key", "available", "reason"),
    [
        ("record", "sk-test", True, None),
        ("auto", "sk-test", True, None),
        ("replay", None, True, None),
        ("record", None, False, "サーバーにAPIキーが設定されていません。"),
        ("auto", None, False, "サーバーがAIを使わない設定（offline）で起動しています。"),
        ("offline", "sk-test", False, "サーバーがAIを使わない設定（offline）で起動しています。"),
    ],
)
def test_llm_status_is_on_only_with_a_key_to_record_with_or_answers_to_replay(
    mode: str, api_key: str | None, available: bool, reason: str | None
) -> None:
    """The screen's ON/OFF of the AI (#581): record mode needs the key; auto mode without one is
    offline; the key itself is never in the answer."""
    from sheltersim.llm.config import LLMSettings

    answer = server.llm_status(LLMSettings(mode=mode, api_key=api_key))

    assert answer == {
        "available": available,
        "mode": LLMSettings(mode=mode, api_key=api_key).effective_mode.value,
        "reason": reason,
    }
    assert "sk-test" not in repr(answer)
