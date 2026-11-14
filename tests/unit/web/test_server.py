"""serve_inspector gives its projects directory the saved-data limit it is started with.

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
