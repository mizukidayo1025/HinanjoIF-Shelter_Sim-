"""The PDF report (#463): grounded prose and a fake Opus that answers it."""

from __future__ import annotations

import json
from collections.abc import Callable
from typing import Any

import pytest


def _prose_document() -> dict[str, Any]:
    return {
        "executive": [
            {
                "text": "待機と支援需要を合わせて確認することで、"
                "運営上の着眼点を整理できる可能性があります。",
                "refs": ["B0.toilet_queue_length"],
            }
        ],
        "interpretation": [
            {
                "text": "待機の継続には処理能力だけでなく、"
                "需要の集中が関係している可能性があります。",
                "refs": ["B0.window.toilet_queue_length"],
            }
        ],
        "hypotheses": [
            {
                "text": "待機と支援需要の重なりが運営上の負担につながる可能性があります。",
                "refs": ["B0.unresolved_support_request_count", "B0.toilet_queue_length"],
            }
        ],
        "proposals": [
            {
                "text": "優先的に、需要の時間帯と設備容量をそろえた比較条件で、"
                "追加検証することを検討してください。",
                "refs": ["B0.toilet_queue_length"],
            },
            {
                "text": "代替案として、担当者の配置条件を変えて、"
                "待機と支援の推移を比較することを検討してください。",
                "refs": ["B0.unresolved_support_request_count"],
            },
        ],
        "limitations": [
            {
                "text": "現地の集計データとの照合と、異なる条件での反復検証が必要です。",
                "refs": ["L"],
            }
        ],
    }


@pytest.fixture
def prose_document() -> Callable[[], dict[str, Any]]:
    """A grounded report analysis that cites only allow-listed evidence."""
    return _prose_document


class _ReportProvider:
    """A fake Opus: answers `document()` (the grounded prose by default), counting its calls and
    keeping their tasks' cache keys."""

    def __init__(self, document: Callable[[], dict[str, Any]] = _prose_document) -> None:
        self.calls = 0
        self.keys: list[str] = []
        self.document = document

    def complete(self, *, model_id, task, **_kwargs):
        from sheltersim.llm.contracts import ProviderResponse

        self.calls += 1
        self.keys.append(task.cache_key(model_id))
        return ProviderResponse(
            json.dumps(self.document(), ensure_ascii=False), model_id, 200, 500, 1
        )


@pytest.fixture
def report_provider() -> type[_ReportProvider]:
    """Make a fake Opus provider that answers prose_document (or the document it is given)."""
    return _ReportProvider
