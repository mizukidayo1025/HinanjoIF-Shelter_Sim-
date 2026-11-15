"""The files of a projects directory that Python writes besides the stored Runs (native/store):
the usage ledgers' records, written atomically under the directory's lock, and the checks of
saved data for secrets and contact details."""

from __future__ import annotations

import errno
import functools
import hashlib
import json
import math
import os
import re
import tempfile
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable, Iterator

try:  # POSIX lock for macOS/Linux.
    import fcntl
except ImportError:  # pragma: no cover - exercised on Windows.
    fcntl = None
    import msvcrt

from sheltersim import native

_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$")
_PRIVATE = re.compile(
    r"api.?key|secret|password|credential|authorization|raw.?prompt|full.?name|email|phone|address",
    re.I,
)
# The top-level domain is letters, so a value with a decimal after "@", such as a
# model signature, is not taken for an address (#187).
EMAIL_ADDRESS = re.compile(r"\b[^\s@]+@[^\s@]+\.[^\W\d_]{2,}\b")
_KEY_OR_BEARER = r"sk-(?:ant-)?[A-Za-z0-9_-]{12,}|Bearer\s+\S+"
_SECRET = re.compile(_KEY_OR_BEARER + "|" + EMAIL_ADDRESS.pattern, re.I)
# _SECRET for text without "@", where its address alternative can never match.
_SECRET_WITHOUT_ADDRESS = re.compile(_KEY_OR_BEARER, re.I)
# Three separated digit groups, so DOIs and dates in provenance are not phone
# numbers (#190). Run artifacts and the saved-Run inspector use the same definition.
# The three middle groups of a UUID (8-4-4-4-12 hex digits) are not one either: the
# screens make Run and branch IDs from crypto.randomUUID(), whose middle groups are all
# digits about once in 200 (#321).
_UUID_MIDDLE = r"(?<=[0-9A-Fa-f]{8}-)\d{4}-\d{4}-\d{4}-[0-9A-Fa-f]{12}(?!\w)"
PHONE_NUMBER = re.compile(
    rf"(?<!\w)(?!{_UUID_MIDDLE})(?:\+\d{{1,3}}[ -])?\d{{2,4}}[- ]\d{{2,4}}[- ]\d{{3,4}}(?!\w)"
)


class StorageError(ValueError):
    """Missing, incompatible, conflicting or unsafe saved artifacts."""

    def __init__(self, message: str, *, code: str | None = None) -> None:
        super().__init__(message)
        candidate = message.split(":", 1)[0]
        self.code = code or (
            candidate if re.fullmatch(r"[a-z][a-z0-9_]{1,80}", candidate) else "storage_conflict"
        )


def canonical(value: Any) -> str:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
    )


def digest(value: Any) -> str:
    encoded = native.canonical_json(value)
    return hashlib.sha256(canonical(value).encode() if encoded is None else encoded).hexdigest()


# Large enough for every key of a large Run: relationship keys alone are one per pair
# of people, and a save reads them all again in order, which a smaller cache never holds.
@functools.lru_cache(maxsize=1 << 18)
def _private_field(key: str) -> bool:
    # Saved records reuse a small set of field names; match each name once (#47).
    return _PRIVATE.search(key) is not None


def _redact(value: str) -> str:
    # _SECRET matches only text with "@", "sk-" or "bearer" (in any case, which
    # casefold() covers). Its address alternative needs "@" and is slow on long text
    # without spaces, as it rescans the text from every word; IDs such as "task-1"
    # contain "sk-", so text without "@" uses the pattern without it.
    if "@" in value:
        value = _SECRET.sub("[redacted]", value)
    elif "sk-" in (folded := value.casefold()) or "bearer" in folded:
        value = _SECRET_WITHOUT_ADDRESS.sub("[redacted]", value)
    return PHONE_NUMBER.sub("[redacted]", value)


# Saved records repeat the same IDs, labels and short texts; redact each one once.
_redact_short = functools.lru_cache(maxsize=16_384)(_redact)


def retry_while_shared(action: Callable[[], Any], *, attempts: int = 50) -> Any:
    """Run a file action, retrying briefly on Windows while another handle shares the file.

    Windows cannot open a file while it is being replaced, nor replace a file another
    thread or process has open; either lasts a moment. POSIX allows both, so there the
    action runs once.
    """
    if os.name != "nt":
        return action()
    for attempt in range(attempts):
        try:
            return action()
        except PermissionError:
            if attempt == attempts - 1:
                raise
            time.sleep(0.01)
    raise AssertionError("unreachable")


def safe_document(value: Any) -> Any:
    """Reject credential-bearing fields; redact conventional secrets/contact text."""
    if type(value) is str:
        return _redact_short(value) if len(value) <= 128 else _redact(value)
    if isinstance(value, dict):
        for key in value:
            if not isinstance(key, str) or _private_field(key):
                raise StorageError("sensitive fields cannot be persisted")
        return {key: safe_document(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [safe_document(item) for item in value]
    if isinstance(value, str):
        return _redact(value)
    if value is None or type(value) is bool:
        return value
    if type(value) is int or (type(value) is float and math.isfinite(value)):
        # Built-in ints and finite floats already satisfy the JSON scalar contract.
        # Avoid constructing a JSON encoder for every coefficient/KPI/history value.
        return value
    if type(value) is float:
        canonical(value)  # Raises for NaN and infinities.
        return value
    raise StorageError("saved data must be finite JSON")


# The saved data one projects directory may hold, unless `sheltersim web --project-storage-gb`
# sets another limit.
DEFAULT_MAX_BYTES = 10_000_000_000


class ProjectStore:
    """Local cloud-disk store; callers provide opaque IDs, never filesystem paths."""

    def __init__(self, root: str | Path, *, max_bytes: int = DEFAULT_MAX_BYTES) -> None:
        self.root = Path(root).resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        if type(max_bytes) is not int or max_bytes <= 0:
            raise StorageError("max_bytes must be positive")
        self.max_bytes = max_bytes
        self._mutex = threading.RLock()

    @contextmanager
    def transaction(self) -> Iterator[None]:
        with self._mutex, (self.root / ".lock").open("a+b") as lock:
            if fcntl is not None:
                fcntl.flock(lock, fcntl.LOCK_EX)
            else:  # Windows msvcrt locks a byte range and needs a byte in the file.
                lock.seek(0, os.SEEK_END)
                if lock.tell() == 0:
                    lock.write(b"\0")
                    lock.flush()
                lock.seek(0)
                # LK_LOCK gives up after 10 tries, a second apart; wait as flock does.
                while True:
                    try:
                        msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
                        break
                    except OSError as exc:
                        if exc.errno != errno.EDEADLK:
                            raise
            try:
                yield
            finally:
                if fcntl is not None:
                    fcntl.flock(lock, fcntl.LOCK_UN)
                else:
                    lock.seek(0)
                    msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)

    def _path(self, *parts: str) -> Path:
        if any(
            not isinstance(part, str)
            or not (_ID.fullmatch(part) or (part.endswith(".json") and _ID.fullmatch(part[:-5])))
            for part in parts
        ):
            raise StorageError("invalid artifact identifier")
        path = self.root.joinpath(*parts)
        if not path.resolve().is_relative_to(self.root) or any(
            p.is_symlink() for p in [path, *path.parents] if p != self.root.parent
        ):
            raise StorageError("artifact path cannot follow symlinks")
        return path

    def _read(self, path: Path) -> dict[str, Any]:
        if path.is_symlink() or not path.resolve().is_relative_to(self.root):
            raise StorageError("unsafe artifact path")
        try:
            value = json.loads(retry_while_shared(lambda: path.read_text(encoding="utf-8")))
        except (FileNotFoundError, json.JSONDecodeError) as exc:
            raise StorageError("saved artifact is missing or corrupt") from exc
        # Conversation v2 adds scalar proof/scope records. Other artifact envelopes
        # remain v1; never accept an unrelated future checkpoint/status format.
        conversation = (
            path.parent.name == "conversations"
            and len(path.parents) > 2
            and path.parents[2].name == "branches"
        )
        versions = {1, 2} if conversation else {1}
        if (
            not isinstance(value, dict)
            or type(value.get("schema_version")) is not int
            or value["schema_version"] not in versions
        ):
            raise StorageError("unsupported saved artifact version; migration required")
        return value

    def _write(self, path: Path, value: dict[str, Any], *, immutable: bool = False) -> None:
        encoded = (canonical(value) + "\n").encode()
        if immutable and path.exists():
            if path.read_bytes() != encoded:
                raise StorageError("immutable saved artifact already exists")
            return
        previous_size = path.stat().st_size if path.exists() else 0
        if self.usage()["bytes"] + len(encoded) - previous_size > self.max_bytes:
            raise StorageError("storage capacity exceeded; export then delete saved runs")
        path.parent.mkdir(parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(prefix=".pending-", dir=path.parent)
        try:
            with os.fdopen(fd, "wb") as stream:
                stream.write(encoded)
                stream.flush()
                os.fsync(stream.fileno())
            if immutable:
                try:
                    os.link(name, path)
                except FileExistsError as exc:
                    if path.read_bytes() != encoded:
                        raise StorageError("immutable saved artifact already exists") from exc
            else:
                retry_while_shared(lambda: os.replace(name, path))
            if os.name != "nt":
                directory_fd = os.open(path.parent, os.O_RDONLY)
                try:
                    os.fsync(directory_fd)
                finally:
                    os.close(directory_fd)
        finally:
            Path(name).unlink(missing_ok=True)

    def usage(self) -> dict[str, int]:
        """The disk usage now."""
        return self._measure_usage()

    def _measure_usage(self) -> dict[str, int]:
        """Measure the regular files on disk now, including other processes' writes.

        Symlinks are neither counted nor followed. Directory entries carry their type,
        so each file costs one stat (#47); a file or directory removed while it is
        measured holds no bytes.
        """
        total = files = 0
        directories = [os.fspath(self.root)]
        while directories:
            try:
                entries = list(os.scandir(directories.pop()))
            except (FileNotFoundError, PermissionError):
                continue
            for entry in entries:
                try:
                    if entry.is_dir(follow_symlinks=False):
                        directories.append(entry.path)
                    elif entry.is_file(follow_symlinks=False):
                        total += entry.stat(follow_symlinks=False).st_size
                        files += 1
                except FileNotFoundError:
                    continue
        return {"bytes": total, "files": files, "limit_bytes": self.max_bytes}

    def create_project(self, project_id: str, *, _internal: bool = False) -> dict[str, Any]:
        if project_id == "runtime-accounts" and not _internal:
            raise StorageError("project_id_reserved_for_usage_accounts")
        with self.transaction():
            document = {"schema_version": 1, "project_id": project_id, "archived": False}
            self._write(self._path(project_id, "project.json"), document, immutable=True)
            return document
