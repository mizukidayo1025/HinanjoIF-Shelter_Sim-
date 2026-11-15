"""Seeking a minute between saved Checkpoints, and a fork there, are exact.

On the shared generated Scenario, so the replay from the saved Checkpoint walks
people on the floor plan, as the Project screen's seek does. Until #137 this ran
on scenario_100_agents, which has neither a floor plan nor facility awareness.
"""

import pytest

from sheltersim.core.engine import TickEngine
from sheltersim.core.world import WorldState
from sheltersim.domain.movement import MOVEMENT_STATE_KEY
from sheltersim.experiments.metrics import MetricsRecorder
from sheltersim.experiments.runner import OfflineSimulationController
from sheltersim.experiments.timeline import TimelineError, TimelineStore, _MetricSink

CHECKPOINT_INTERVAL = 20
# Between the Checkpoints at 20 and 40, while people walk on the floor plan.
SEEK_MINUTE = 28
COMPUTED_UNTIL = 45


def test_non_checkpoint_seek_and_noop_fork_are_exact(tmp_path, generated_scenario, saved_timeline):
    scenario = generated_scenario
    store, timeline = saved_timeline(tmp_path, scenario)
    timeline.advance("main", COMPUTED_UNTIL, checkpoint_interval=CHECKPOINT_INTERVAL)
    reference = WorldState.from_scenario(scenario)
    TickEngine(
        reference,
        phase_handler=OfflineSimulationController(reference, _MetricSink(MetricsRecorder())),
    ).run(steps=SEEK_MINUTE)
    assert reference.model_state[MOVEMENT_STATE_KEY]["journeys"]
    assert timeline.seek("main", SEEK_MINUTE).world.canonical_json() == reference.canonical_json()
    parent_before = store.load_branch("p", "r", "main")[1].to_json()
    timeline.fork("main", SEEK_MINUTE, "child")
    child = timeline.advance("child", COMPUTED_UNTIL)
    assert (
        child.world.canonical_json() == timeline.seek("main", COMPUTED_UNTIL).world.canonical_json()
    )
    assert store.load_branch("p", "r", "main")[1].to_json() == parent_before
    with pytest.raises(TimelineError, match="computed range"):
        timeline.seek("main", COMPUTED_UNTIL + 1)


def test_a_forked_branch_reads_the_rows_a_fresh_ingest_derives(
    tmp_path, generated_scenario, saved_timeline
):
    """#256: a forked branch stores no rows before its fork minute and reads its
    parent's instead; what it reads equals the rows derived from its head from scratch."""
    import sqlite3

    from sheltersim.storage.history_index import (
        _R_KEY,
        HistoryIndex,
        _name_id,
        _scope,
        _segments,
    )

    store, timeline = saved_timeline(tmp_path, generated_scenario)
    timeline.advance("main", COMPUTED_UNTIL, checkpoint_interval=CHECKPOINT_INTERVAL)
    timeline.fork("main", SEEK_MINUTE, "child")
    _, head = store.load_branch("p", "r", "child")
    index = HistoryIndex(store, "p", "r")
    index.ingest("fresh", head)
    db = sqlite3.connect(index.path)

    def rows(branch):
        segments = [(_name_id(db, name), limit) for name, limit in _segments(db, branch)]
        scope, args = _scope(segments)
        # Payloads compare as text: compressed bytes may differ between zlib builds (#80),
        # and a row made from an EventLog record is made again from it (#138).
        named = db.cursor()
        named.row_factory = sqlite3.Row
        found = named.execute(
            f"SELECT k.name,{_R_KEY} AS key,r.minute,r.branch,r.kind,r.position,r.payload,"
            "r.payload_hash "
            f"FROM records r JOIN kinds k ON k.code=r.kind WHERE {scope}",
            args,
        ).fetchall()
        texts = index.payload_texts(db, found)
        tables = {
            "records": sorted(
                (row[0], row[1], row[2], text) for row, text in zip(found, texts, strict=True)
            )
        }
        for table, column in (("entities", "entity"), ("tokens", "token"), ("links", "event")):
            tables[table] = sorted(
                db.execute(
                    f"SELECT k.name,{_R_KEY},n.name FROM {table} e JOIN records r ON r.id=e.record "
                    f"JOIN kinds k ON k.code=r.kind JOIN names n ON n.id=e.{column} WHERE {scope}",
                    args,
                )
            )
        heads = db.execute("SELECT minute,counts FROM heads WHERE branch=?", (branch,))
        tables["heads"] = heads.fetchall()
        return tables

    child, fresh = rows("child"), rows("fresh")
    assert child["heads"] == fresh["heads"]
    for table in ("records", "entities", "tokens", "links"):
        assert set(fresh[table]) <= set(child[table])
    # Besides, it reads the decisions the parent recorded while computing before the
    # fork minute, which live only in the index and no Checkpoint (#213).
    extra = set(child["records"]) - set(fresh["records"])
    assert extra and {(kind, minute < SEEK_MINUTE) for kind, _, minute, _ in extra} == {
        ("actions", True)
    }
    stored = db.execute(
        "SELECT minute FROM records WHERE branch=?", (_name_id(db, "child"),)
    ).fetchall()
    assert all(minute >= SEEK_MINUTE - 1 for (minute,) in stored)


def test_a_forked_branch_answers_history_before_its_fork_through_its_parent(
    tmp_path, generated_scenario, saved_timeline
):
    """#256: a search of a forked branch before its fork minute reads its parent's
    rows; conversation turns stay with their branch; a rebuilt branch stores its own."""
    import sqlite3

    from sheltersim.contracts.workspace import HistoryQuery
    from sheltersim.storage.history_index import HistoryIndex

    store, timeline = saved_timeline(tmp_path, generated_scenario)
    timeline.advance("main", COMPUTED_UNTIL, checkpoint_interval=CHECKPOINT_INTERVAL)
    index = HistoryIndex(store, "p", "r")
    index.add_turn("main", {"turn_id": "turn-1", "minute": 10, "conversation_id": "talk"})
    timeline.fork("main", SEEK_MINUTE, "child")

    def items(branch, **query):
        reference = store.branch_reference("p", "r", branch)
        found, cursor = [], None
        while True:
            page = index.search(
                branch,
                HistoryQuery(limit=100, cursor=cursor, **query),
                reference.checkpoint_hash,
                reference.computed_until,
            )
            found.extend(page["items"])
            cursor = page["next_cursor"]
            if cursor is None:
                return found

    before_fork = {"kind": "events", "start": 0, "end": SEEK_MINUTE - 1}
    expected = items("main", **before_fork)
    assert expected and items("child", **before_fork) == expected
    talk = {"kind": "conversations", "start": 0, "end": COMPUTED_UNTIL}
    assert len(items("main", **talk)) == 1
    assert items("child", **talk) == []

    _, head = store.load_branch("p", "r", "child")
    index.ingest("child", head, rebuild=True)
    with sqlite3.connect(index.path) as db:
        assert db.execute("SELECT COUNT(*) FROM lineage WHERE branch='child'").fetchone() == (0,)
    assert items("child", **before_fork) == expected


def test_frames_read_while_a_save_is_indexed_match_the_frames_after(
    tmp_path, monkeypatch, saved_timeline
):
    """#259: a save moves the branch pointer before it indexes the new head; a frame
    read in between is the same as one read after the save."""
    from sheltersim.storage.history_index import HistoryIndex

    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "offline")
    store, timeline = saved_timeline(tmp_path)
    timeline.advance("main", 30)
    minutes = (0, 15, 30, 45, 60)
    during = {}
    original = HistoryIndex.ingest

    def ingest(self, branch, checkpoint, **options):
        if checkpoint.simulation_minute == 60:
            assert store.branch_reference("p", "r", "main").computed_until == 60
            reader = TimelineStore(store, "p", "r")
            during.update({minute: reader.frame("main", minute) for minute in minutes})
        return original(self, branch, checkpoint, **options)

    monkeypatch.setattr(HistoryIndex, "ingest", ingest)
    timeline.advance("main", 60)
    monkeypatch.setattr(HistoryIndex, "ingest", original)
    assert set(during) == set(minutes)
    for minute in minutes:
        assert during[minute] == TimelineStore(store, "p", "r").frame("main", minute)
