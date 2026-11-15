"""The files of a stored Run on a disk, from Python (`sheltersim_native.rules.run_files`, #426).

The store (`native/store`) writes and reads the browser's folders: the records, a branch's
`journal.jsonl` and `data.pack` of format v5, computed from its head in chunks. A holder of a
branch's lock keeps others out, in this process and in another. A writer process killed while it
holds its branch leaves the lock free and its record at its last head; a line it left cut short
is not read, and going on from the head cuts it off and reaches the Run computed straight through.
A reader in another process reads every piece of the lines it sees while the writer appends. That
the store writes the browser's Runs byte for byte is `cargo test -p sheltersim_store`'s.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import threading
from pathlib import Path

import pytest
from support.paths import CORE_TEST_DATA

from sheltersim import native

# Run on every pull request on Windows and macOS too (pytest --os-subset, #380): files flushed and
# cut, locks shared with Python's, a process killed while it holds one.
pytestmark = pytest.mark.os_dependent

FILES = native.rules("run_files")
PLAIN = native.rules("plain")
TICK = (CORE_TEST_DATA / "tick_core.json").read_bytes()
WORLD = "small_replay"
VERSIONS = json.dumps({"engine:model": "test"})
BRANCH_BUSY = "^run_worker_busy: another process writes this branch$"


def _store(root: Path) -> FILES.ProjectFiles:
    """A store with Project `p`, Run `r` of tick_core's small_replay world, and its branch `main`
    at minute 0."""
    files = FILES.ProjectFiles(root)
    duration = PLAIN.PlainRun.from_worlds(TICK, WORLD).duration()
    files.create_project("p")
    scenario = json.dumps({"duration_minutes": duration, "id": "s", "seed": 1}).encode()
    files.save_scenario("p", scenario, hashlib.sha256(scenario).hexdigest())
    files.create_run("p", "r", "s", 1, 1, VERSIONS, b'{"schema_version":1}')
    with files.claim_branch("p", "r", "main") as lease:
        files.start_branch(lease, "p", "r", "main", TICK, WORLD)
    return files


def _compute(files, lease, target: int, ticks: int = 60) -> str:
    computation = files.go_on(lease, "p", "r", "main", target)
    while (status := computation.chunk(files, ticks)) is None:
        pass
    computation.end(files, status)
    return status


def test_a_branch_is_computed_from_its_head_and_read_back(tmp_path):
    files = _store(tmp_path)
    record = json.loads(files.branch_record("p", "r", "main"))
    assert record["computed_until"] == 0 and "status" not in record
    with files.claim_branch("p", "r", "main") as lease:
        assert _compute(files, lease, 700) == "paused"
    record = json.loads(files.branch_record("p", "r", "main"))
    assert (record["computed_until"], record["status"]) == (700, "paused")
    assert json.loads(files.run_record("p", "r"))["status"] == "paused"

    reader = files.reader("p", "r", "main")
    heads = json.loads(reader.heads_json())
    kinds = [(head["kind"], head["minute"]) for head in heads]
    assert kinds == [("export", 0), ("saved", 360), ("saved", 700)]
    assert heads[-1]["head"] == record["checkpoint_hash"]
    assert not reader.torn() and reader.save_every() == FILES.SAVE_EVERY
    assert json.loads(reader.model_versions_json()) == json.loads(VERSIONS)
    assert reader.export_bytes() == TICK
    assert reader.records("decisions")
    verified = json.loads(reader.verify_json())
    assert verified["mismatches"] == [] and not verified["complete"]
    # A seek restores the saved point at 360 and computes on: the world of the Run straight through.
    run, start, base = reader.seek(500)
    straight = PLAIN.PlainRun.from_worlds(TICK, WORLD)
    straight.advance(500)
    assert (start, run.minute(), run.digests_json()) == (360, 500, straight.digests_json())
    assert base == heads[1]["index"]

    raw = b"the bytes of a piece" * 50
    assert FILES.inflate_raw(FILES.deflate_raw(raw), len(raw)) == raw
    with pytest.raises(ValueError, match="holds 1000 bytes, not the 999"):
        FILES.inflate_raw(FILES.deflate_raw(raw), 999)


# The writer's process: it goes on with `main` toward minute 1000, says "ready" once its head is
# past 360 and waits for "go", then computes to 1000 (paused), says so, and waits to be killed with
# its lease held and its record still `running`.
WRITER = """
import sys
from sheltersim import native
files = native.rules("run_files").ProjectFiles(sys.argv[1])
lease = files.claim_branch("p", "r", "main")
computation = files.go_on(lease, "p", "r", "main", 1000)
ready = False
while (status := computation.chunk(files, 10)) is None:
    if not ready and computation.head()[0] >= 360:
        ready = True
        print("ready", flush=True)
        sys.stdin.readline()
print(status, computation.head()[0], flush=True)
sys.stdin.readline()
"""


# The writer's process: it goes on with `main` toward minute 1000, says "ready" once its head is
# past 360 and waits for "go", then computes to 1000 (paused), says so, and waits to be killed with
# its lease held and its record still `running`.
WRITER = """
import sys
from sheltersim import native
files = native.rules("run_files").ProjectFiles(sys.argv[1])
lease = files.claim_branch("p", "r", "main")
computation = files.go_on(lease, "p", "r", "main", 1000)
ready = False
while (status := computation.chunk(files, 10)) is None:
    if not ready and computation.head()[0] >= 360:
        ready = True
        print("ready", flush=True)
        sys.stdin.readline()
print(status, computation.head()[0], flush=True)
sys.stdin.readline()
"""


def test_a_killed_writer_leaves_its_lock_free_and_its_cut_line_unread(tmp_path):
    files = _store(tmp_path / "store")
    folder = Path(files.branch_folder("p", "r", "main"))
    writer = subprocess.Popen(
        [sys.executable, "-c", WRITER, str(tmp_path / "store")],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert writer.stdout.readline().strip() == "ready"
        with pytest.raises(ValueError, match=BRANCH_BUSY):
            files.claim_branch("p", "r", "main")
        # Read while the writer appends, until it says where it stopped.
        said: list[str] = []
        listener = threading.Thread(target=lambda: said.append(writer.stdout.readline().strip()))
        listener.start()
        writer.stdin.write("go\n")
        writer.stdin.flush()
        reads, lines = 0, set()
        while listener.is_alive() or reads == 0:
            reader = files.reader("p", "r", "main")
            assert json.loads(reader.verify_json())["mismatches"] == []
            lines.add(len(json.loads(reader.lines_json())))
            reads += 1
        listener.join()
        assert said == ["paused 1000"]
    finally:
        writer.kill()
        writer.wait()
    assert reads >= 1 and max(lines) > 3

    # Killed: its record at its last head, still running; a line it was writing cut short.
    record = json.loads(files.branch_record("p", "r", "main"))
    assert (record["computed_until"], record["status"]) == (1000, "running")
    journal = (folder / "journal.jsonl").read_bytes()
    last = journal[journal.rstrip(b"\n").rfind(b"\n") + 1 :]
    with (folder / "data.pack").open("ab") as data:
        data.write(b"bytes of a piece no line points at")
    with (folder / "journal.jsonl").open("ab") as cut:
        cut.write(last[: len(last) // 2])
    reader = files.reader("p", "r", "main")
    assert reader.torn()
    assert len(json.loads(reader.lines_json())) == journal.count(b"\n")
    verified = json.loads(reader.verify_json())
    assert verified["torn"] and not verified["complete"] and verified["mismatches"] == []
    assert files.recover_interrupted() == [("p", "r", "main")]
    assert json.loads(files.branch_record("p", "r", "main"))["status"] == "interrupted"
    assert json.loads(files.run_record("p", "r"))["status"] == "interrupted"

    # Going on cuts the line off and reaches the Run computed straight through.
    with files.claim_branch("p", "r", "main") as lease:
        assert _compute(files, lease, 1440) == "completed"
    reader = files.reader("p", "r", "main")
    verified = json.loads(reader.verify_json())
    assert not reader.torn() and verified["complete"] and verified["verified"]
    straight = _store(tmp_path / "straight")
    with straight.claim_branch("p", "r", "main") as lease:
        assert _compute(straight, lease, 1440) == "completed"
    head = json.loads(files.branch_record("p", "r", "main"))["checkpoint_hash"]
    assert head == json.loads(straight.branch_record("p", "r", "main"))["checkpoint_hash"]


def test_the_server_reads_the_files_an_answer_needs_and_exports_a_project(tmp_path):
    files = _store(tmp_path)
    with files.claim_branch("p", "r", "main") as lease:
        assert _compute(files, lease, 400) == "paused"
    # What the core's answers read (#427): the records by folder, a journal up to its last line.
    assert [name for name, _ in files.projects_listing()] == ["p"]
    run, branches = files.run_listing("p", "r")
    assert json.loads(run)["run_id"] == "r" and [name for name, _ in branches] == ["main"]
    project, scenarios, runs = files.project_listing("p", True)
    assert json.loads(project)["project_id"] == "p"
    assert [(scenario, [name for name, _ in each]) for scenario, each in scenarios] == [
        ("s", ["v1.json", "v1.scenario.json"])
    ]
    assert [name for name, _, _ in runs] == ["r"]
    assert files.run_file("p", "r", "evidence.json") == b'{"schema_version":1}'
    journal = files.branch_file("p", "r", "main", "journal.jsonl")
    assert journal == (Path(files.branch_folder("p", "r", "main")) / "journal.jsonl").read_bytes()
    assert files.branch_file("p", "r", "main", "report.json") is None
    with pytest.raises(ValueError, match="no file data.pack of a branch is read"):
        files.branch_file("p", "r", "main", "data.pack")
    reader = files.reader("p", "r", "main")
    kinds = [kind for kind, _ in reader.sample_pieces()]
    assert kinds[0] == "sample_tables" and set(kinds) == {"sample_tables", "sample_block"}
    assert reader.end_samples() is None

    # A Run is running while any of its branches is.
    with files.transaction():
        files.set_branch_status("p", "r", "main", "running")
        assert json.loads(files.run_record("p", "r"))["status"] == "running"
        files.set_branch_status("p", "r", "main", "paused")
    assert json.loads(files.run_record("p", "r"))["status"] == "paused"

    # The bundle the browser imports: the records and the stored Run, not the server's lock.
    bundle = files.export_project("p", '{"tool": "test"}')
    assert bundle[:16] == b"\x89SHELTERSIM-PRJ\n" and FILES.BUNDLE_EXTENSION == ".shelterproj"
    length = int.from_bytes(bundle[20:24], "big")
    manifest = json.loads(bundle[56 : 56 + length])
    assert manifest["source"] == {"tool": "test"}
    paths = [entry["path"] for entry in manifest["entries"]]
    assert "runs/r/branches/main/journal.jsonl" in paths
    assert not any("worker-lock" in path for path in paths)


def test_a_run_computed_with_an_llm_keeps_its_tape_and_a_seek_answers_from_it(tmp_path):
    """A computation given an LLM link (#428, the server's Run with a Provider) keeps each call in
    the stored Run's LLM tape; a seek between saved points answers the ticks from the tape, never
    from a provider, and reaches the Run's world (#427)."""
    import dataclasses

    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmarks"))
    from gate_scenarios import ScriptedProvider, _llm_settings, generated, with_rumors

    from sheltersim.llm.cache import ResponseTape
    from sheltersim.llm.config import LLMMode
    from sheltersim.llm.link import RouterLink
    from sheltersim.llm.provider import LLMRouter
    from sheltersim.web.stored_runs import minute_zero_entry

    limits = json.dumps(
        {
            "social": {
                "cooldown_minutes": 5,
                "per_agent_per_day": 40,
                "global_per_day": 120,
                "complaint_wait_minutes": 5,
            }
        }
    ).encode()
    scenario = with_rumors(generated(evacuees=24, minutes=240, staff_count=1))
    entry = json.loads(minute_zero_entry(scenario.canonical_json().encode()))
    for agent in entry["world"]["agents"]:
        agent["stress"] = 0.9
    export = json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()

    def link() -> RouterLink:
        settings = dataclasses.replace(_llm_settings(LLMMode.RECORD))
        return RouterLink(LLMRouter(settings, provider=ScriptedProvider(), tape=ResponseTape()))

    files = FILES.ProjectFiles(tmp_path)
    files.create_project("p")
    text = json.dumps({"duration_minutes": 240, "id": "s", "seed": 1}).encode()
    files.save_scenario("p", text, hashlib.sha256(text).hexdigest())
    files.create_run("p", "r", "s", 1, 1, VERSIONS, b'{"schema_version":1}')
    with files.claim_branch("p", "r", "main") as lease:
        files.start_branch(lease, "p", "r", "main", export, save_every=60)
        computation = files.go_on(lease, "p", "r", "main", 240)
        computation.set_llm(link(), limits)
        while computation.chunk(files, 25) is None:
            pass
    reader = files.reader("p", "r", "main")
    tape = reader.records("llm_tape")
    assert tape
    run, start, _ = reader.seek(150, llm_limits=limits)
    assert (start, run.minute()) == (120, 150)
    straight = PLAIN.PlainRun(export)
    straight.set_llm(link(), limits)
    straight.advance(150)
    assert run.digests_json() == straight.digests_json()


def test_a_run_whose_llm_calls_are_accounted_computes_without_waiting_for_its_own_lock(
    tmp_path, monkeypatch
):
    """The server's computation (`worker_process.finish`) of a Run with a Provider (#465): the
    router behind the core's port reserves each call in the Run's usage ledger, which takes the
    store's transaction. The chunk's ticks run outside that transaction, so the call does not wait
    for the lock its own computation holds; the Run completes and its tape holds the calls."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmarks"))
    from gate_scenarios import ScriptedProvider, generated, with_rumors

    from sheltersim.web.stored_runs import minute_zero_entry
    from sheltersim.web.worker_process import finish

    scenario = with_rumors(generated(evacuees=24, minutes=120, staff_count=1))
    entry = json.loads(minute_zero_entry(scenario.canonical_json().encode()))
    for agent in entry["world"]["agents"]:
        agent["stress"] = 0.9
    export = json.dumps(entry, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    store = ProjectStore(tmp_path)
    files = FILES.ProjectFiles(tmp_path)
    files.create_project("p")
    text = json.dumps({"duration_minutes": 120, "id": "s", "seed": 1}).encode()
    files.save_scenario("p", text, hashlib.sha256(text).hexdigest())
    files.create_run("p", "r", "s", 1, 1, VERSIONS, b'{"schema_version":1}')
    with files.claim_branch("p", "r", "main") as lease:
        files.start_branch(lease, "p", "r", "main", export, save_every=60)
    llm = {"schema_version": 1, "mode": "record", "execution_purpose": None}
    store._write(store._path("p", "runs", "r", "llm.json"), llm)
    monkeypatch.setenv("SHELTERSIM_LLM_MODE", "record")
    provider = ScriptedProvider()
    monkeypatch.setattr("sheltersim.llm.provider.AnthropicProvider", lambda _key: provider)

    outcome: list[str] = []
    lease = files.claim_branch("p", "r", "main")
    # A daemon thread: on the old code it waits for the lock for ever.
    worker = threading.Thread(
        target=lambda: outcome.append(finish(files, lease, "p", "r", "main", 120, lambda: False)),
        daemon=True,
    )
    worker.start()
    worker.join(timeout=120)
    assert not worker.is_alive(), "the computation waits for the store's lock it holds"
    assert outcome == ["completed"]
    tape = [json.loads(item) for item in files.reader("p", "r", "main").records("llm_tape")]
    assert any(item["outcome"] == "recorded" for item in tape)
