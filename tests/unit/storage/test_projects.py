"""Project storage keeps a saved answer's proofs and lets one worker write a Run at a time;
a restart keeps the exact Checkpoint, and stale saves, path traversal and edits of a
started Run are refused.

Saving a Checkpoint validates and detaches it once and refuses mutated or sensitive
data before anything is written; saved Runs over time are checked in
tests/integration/test_checkpoint_saves.py. The storage usage counts the regular files
on disk at each call, and a write over the capacity is refused.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from sheltersim.storage.projects import ProjectStore, StorageError, safe_document

try:
    import fcntl
except ImportError:  # Windows
    fcntl = None

# Windows has no fcntl and no os.O_NOFOLLOW; msvcrt.locking locks a byte range per file
# handle. Linux open-file-description locks give the same per-handle behaviour, so the
# windows_like fixture (tests/support/storage.py) simulates Windows with them.
needs_ofd_locks = pytest.mark.skipif(
    not hasattr(fcntl, "F_OFD_SETLK"), reason="needs Linux open-file-description locks"
)


@pytest.mark.parametrize(
    "value", [None, True, False, 0, -1, 10**30, 0.0, -0.0, 0.12345678901234567]
)
def test_privacy_scalar_fast_path_keeps_exact_json_values(value):
    assert json.dumps(safe_document(value), allow_nan=False) == json.dumps(value, allow_nan=False)


@pytest.mark.parametrize("value", [float("inf"), -float("inf"), float("nan")])
def test_privacy_scalar_fast_path_still_rejects_nonfinite_numbers(value):
    with pytest.raises(ValueError):
        safe_document({"values": [value]})


def test_usage_counts_regular_files_without_following_symlinks(tmp_path):
    store = ProjectStore(tmp_path / "store")
    (store.root / "p" / "runs").mkdir(parents=True)
    (store.root / "p" / "runs" / ".hidden").write_bytes(b"12345")
    (store.root / "p" / "a.json").write_bytes(b"1")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "big").write_bytes(b"0" * 100)
    (store.root / "p" / "linked").symlink_to(outside, target_is_directory=True)
    (store.root / "p" / "b.json").symlink_to(outside / "big")
    # .lock is created empty by the first transaction.
    assert store.usage() == {"bytes": 6, "files": 2, "limit_bytes": store.max_bytes}


def test_usage_sees_another_stores_writes_at_once(tmp_path):
    """Usage is measured at each call, never cached, so another process's file counts (#47)."""
    store = ProjectStore(tmp_path)
    store.create_project("a")
    before = store.usage()
    ProjectStore(tmp_path).create_project("p")
    after = store.usage()
    size = (tmp_path / "p" / "project.json").stat().st_size
    assert (after["bytes"], after["files"]) == (before["bytes"] + size, before["files"] + 1)


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="Windows keeps each entry's size from the directory listing, so a file "
    "removed while it is measured still counts",
)
def test_usage_skips_a_file_removed_while_it_is_measured(tmp_path, monkeypatch):
    store = ProjectStore(tmp_path)
    (tmp_path / "kept").write_bytes(b"12")
    (tmp_path / ".pending-x").write_bytes(b"1234")
    scandir = os.scandir

    def listing_then_removal(path):
        entries = list(scandir(path))
        (tmp_path / ".pending-x").unlink(missing_ok=True)
        return iter(entries)

    monkeypatch.setattr(os, "scandir", listing_then_removal)
    assert store.usage()["bytes"] == 2


def test_a_write_over_the_capacity_is_refused_and_leaves_the_file(tmp_path):
    store = ProjectStore(tmp_path)
    store.create_project("p")
    path = store._path("p", "note.json")
    store._write(path, {"schema_version": 1, "text": "a"})
    store.max_bytes = store.usage()["bytes"] + 9
    with pytest.raises(StorageError, match="capacity exceeded"):
        store._write(path, {"schema_version": 1, "text": "a" * 11})
    assert store._read(path)["text"] == "a"
    # Replacing a file counts only the bytes it adds.
    store._write(path, {"schema_version": 1, "text": "a" * 10})
    assert store._read(path)["text"] == "a" * 10


def test_a_transaction_on_windows_keeps_waiting_for_a_long_held_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """msvcrt gives up after about 10 seconds with EDEADLK; another process may hold on longer."""
    import errno
    import types

    import sheltersim.storage.projects as projects_module

    calls = []

    def locking(fd: int, mode: int, size: int) -> None:
        calls.append(mode)
        if len(calls) < 3:
            raise OSError(errno.EDEADLK, "Resource deadlock avoided")

    monkeypatch.setattr(projects_module, "fcntl", None)
    monkeypatch.setattr(
        projects_module,
        "msvcrt",
        types.SimpleNamespace(LK_LOCK=1, LK_NBLCK=2, LK_UNLCK=0, locking=locking),
        raising=False,
    )
    with ProjectStore(tmp_path).transaction():
        assert calls == [1, 1, 1]
    assert calls == [1, 1, 1, 0]


def test_a_transaction_on_windows_reports_other_lock_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import types

    import sheltersim.storage.projects as projects_module

    def locking(fd: int, mode: int, size: int) -> None:
        raise PermissionError(13, "locked")

    monkeypatch.setattr(projects_module, "fcntl", None)
    monkeypatch.setattr(
        projects_module,
        "msvcrt",
        types.SimpleNamespace(LK_LOCK=1, LK_NBLCK=2, LK_UNLCK=0, locking=locking),
        raising=False,
    )
    with pytest.raises(PermissionError):
        with ProjectStore(tmp_path).transaction():
            pass


def test_phone_numbers_are_redacted_but_dois_and_dates_are_not():
    """Run artifacts share this definition; a broader one blanked model references (#190)."""
    from sheltersim.storage.projects import safe_document

    for text in ("call 090-1234-5678", "03-1234-5678", "+81 90 1234 5678"):
        assert safe_document(text) != text
    for text in (
        "DeGroot (1974), doi:10.1080/01621459.1974.10480137",
        "https://link.springer.com/article/10.1007/s10458-012-9201-1",
        "doi:10.1111/1467-8721.ep10770953",
        "accessed 2026-10-28",
    ):
        assert safe_document(text) == text


def test_uuid_ids_whose_middle_groups_are_digits_are_not_phone_numbers():
    """The screens make IDs from crypto.randomUUID(); about one in 200 has three
    all-digit middle groups, which were taken for a phone number (#321)."""
    from sheltersim.storage.projects import safe_document

    for text in (
        "branch-3f2a9c1e-4127-4823-9012-abcdef123456",
        "RUN-3F2A9C1E-4127-4823-9012-123456789012",
        "branch-00000000-4000-8000-0000-000000000001",
    ):
        assert safe_document(text) == text
    # Not a UUID: a short last group, a phone number after a hex group, four groups.
    assert safe_document("3f2a9c1e-4127-4823-9012-abcdef12345") != (
        "3f2a9c1e-4127-4823-9012-abcdef12345"
    )
    assert safe_document("3f2a9c1e-090-1234-5678") == "3f2a9c1e-[redacted]"
    assert safe_document("1234-5678-9012-3456") == "[redacted]-3456"


def test_model_signatures_are_kept_while_email_addresses_are_redacted():
    """A decimal after "@" in a model signature was taken for an address (#187)."""
    from sheltersim.storage.projects import safe_document

    signature = "distance-sampled-proximity-v1@1:state1:distance=1.5:interval=15:pairs=500"
    assert safe_document("contact name@example.com today") == "contact [redacted] today"
    assert safe_document(signature) == signature


def test_repeated_short_and_long_texts_and_text_subclasses_are_redacted_alike() -> None:
    """Short texts are redacted once and remembered (#223); every text gets the same result."""
    short = "call 090-1234-5678"
    long = "note " * 30 + "name@example.com"

    class Label(str):
        pass

    for _ in range(2):
        assert safe_document({"a": short, "b": [long, (short,)], "c": Label(short)}) == {
            "a": "call [redacted]",
            "b": ["note " * 30 + "[redacted]", ["call [redacted]"]],
            "c": "call [redacted]",
        }
    with pytest.raises(StorageError, match="sensitive fields"):
        safe_document({"note": short, 1: "x"})


def test_texts_are_redacted_as_by_the_secret_and_phone_patterns() -> None:
    """Text without "@" skips the address pattern, and text without "@", "sk-" or "bearer"
    the whole secret pattern; results are unchanged."""
    from sheltersim.storage.projects import _SECRET, PHONE_NUMBER

    texts = [
        '{"realized_events":["action-lifecycle-38460"],"busy_staff":[]}' * 40,
        '{"staff_tasks":["task-1","task-2"]}' * 40,  # "sk-" without "@"
        "staff-task-abcdefghijklmnop",
        "sk-abcdefghijklmnop@example.com and Bearer name@example.com",
        "token sk-ant-abcdefghijklmnop end",
        "SK-ABCDEFGHIJKLMNOP",
        "ſK-abcdefghijklmnop",  # long s and Kelvin sign match "sk" ignoring case
        "Authorization: BEARER abc.def",
        "beаrer abc",  # Cyrillic a: not "bearer"
        "write to name@example.com or call 090-1234-5678",
        "straße k- 03-1234-5678",
        "no secrets here",
        "",
    ]
    for text in texts:
        expected = PHONE_NUMBER.sub("[redacted]", _SECRET.sub("[redacted]", text))
        assert safe_document(text) == expected
        assert safe_document([text]) == [expected]


def test_a_file_shared_for_a_moment_on_windows_is_tried_again(monkeypatch):
    """Windows cannot open a file being replaced, nor replace one open elsewhere; a
    status read during a save answered HTTP 500 there."""
    from sheltersim.storage import projects

    monkeypatch.setattr(projects.os, "name", "nt")
    monkeypatch.setattr(projects.time, "sleep", lambda _seconds: None)
    tries = []

    def shared_twice():
        tries.append(1)
        if len(tries) < 3:
            raise PermissionError("the file is being used by another process")
        return "read"

    assert projects.retry_while_shared(shared_twice) == "read"
    assert len(tries) == 3

    def always_shared():
        raise PermissionError("the file is being used by another process")

    with pytest.raises(PermissionError):
        projects.retry_while_shared(always_shared, attempts=3)


_PRIVATE_DOCUMENTS = [
    {"api_key": "x"},
    {"nested": [{"Full Name": 1}]},
    ["call 090-1234-5678"],
    ["+81 90-1234-5678"],
    ["０９０-１２３４-５６７８"],
    {"note": "write to person@example.org today"},
    ["token sk-abcdefghijkl0123"],
    ["token sk-ant-abcdefgh"],
    ["Token SK-ABCDEFGHIJKL"],
    ["token ſk-abcdefghijkl"],
    ["token sK-abcdefghijkl"],
    ["BEARER abc"],
    ["line\nBearer\tabc"],
    ['quoted "person@example.org"'],
    ["あ 090-1234-5678"],
]
_CLEAN_DOCUMENTS = [
    {"task-1": "task-1", "id": "task-abcdefghijk", "text": "あいう"},
    [1.5, -2, None, True, "2026-10-31", 'a "quoted" \\ \n string'],
    ["bearing", "1-2", "١", "minute 12-34"],
    {"a": {"b": [{"c": "no-digits - here"}]}},
    [],
    {},
]
