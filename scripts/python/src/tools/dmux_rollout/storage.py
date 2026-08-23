from __future__ import annotations

import fcntl
import json
import os
import stat
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from tools.dmux_rollout.errors import StateError
from tools.dmux_rollout.model import Release, require_release_id, utc_now

MANIFEST = "release.json"
JOURNAL = "journal.jsonl"
LOCK = ".lock"
ACTIVE = "active.json"


def default_state_root() -> Path:
    home = Path.home()
    if os.uname().sysname == "Darwin":
        return home / "Library" / "Application Support" / "dmux" / "rollouts"
    state = Path(os.environ.get("XDG_STATE_HOME", home / ".local" / "state"))
    if not state.is_absolute():
        raise StateError("XDG_STATE_HOME must be absolute")
    return state / "dmux" / "rollouts"


def _require_private_dir(path: Path, *, create: bool) -> None:
    if not path.is_absolute():
        raise StateError(f"state directory must be absolute: {path}")
    if not path.exists() and create:
        path.mkdir(mode=0o700, parents=True)
        os.chmod(path, 0o700)
    try:
        metadata = path.lstat()
    except FileNotFoundError as error:
        raise StateError(f"state directory does not exist: {path}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise StateError(f"state path is not a real directory: {path}")
    if metadata.st_uid != os.getuid():
        raise StateError(f"state directory is not owned by uid {os.getuid()}: {path}")
    if stat.S_IMODE(metadata.st_mode) != 0o700:
        raise StateError(f"state directory must have mode 0700: {path}")
    if path.resolve(strict=True) != path:
        raise StateError(f"state directory traverses a symlink: {path}")


def _require_private_file(fd: int, name: str) -> None:
    metadata = os.fstat(fd)
    if not stat.S_ISREG(metadata.st_mode):
        raise StateError(f"{name} is not a regular file")
    if metadata.st_uid != os.getuid():
        raise StateError(f"{name} is not owned by uid {os.getuid()}")
    if metadata.st_nlink != 1:
        raise StateError(f"{name} must have exactly one hard link")
    if stat.S_IMODE(metadata.st_mode) & 0o077:
        raise StateError(f"{name} is group/world accessible")


class RolloutStore:
    def __init__(self, root: Path | None = None):
        self.root = (root or default_state_root()).absolute()

    def initialize(self) -> None:
        _require_private_dir(self.root, create=True)
        releases = self.root / "releases"
        _require_private_dir(releases, create=True)

    @contextmanager
    def exclusive(self) -> Iterator[None]:
        self.initialize()
        flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_CLOEXEC", 0) | os.O_NOFOLLOW
        fd = os.open(self.root / LOCK, flags, 0o600)
        try:
            _require_private_file(fd, LOCK)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise StateError("another dmux-rollout process holds the release lock") from error
            os.ftruncate(fd, 0)
            os.write(fd, f"{os.getpid()}\n".encode())
            os.fsync(fd)
            yield
        finally:
            os.close(fd)

    def release_dir(self, release_id: str, *, create: bool = False) -> Path:
        release_id = require_release_id(release_id)
        path = self.root / "releases" / release_id
        _require_private_dir(path, create=create)
        return path

    def release_ids(self) -> list[str]:
        """Every release with a manifest, oldest first (ids are date-prefixed)."""
        releases = self.root / "releases"
        if not releases.is_dir():
            return []
        return sorted(entry.name for entry in releases.iterdir() if (entry / MANIFEST).is_file())

    def manifest_path(self, release_id: str) -> Path:
        return self.release_dir(release_id) / MANIFEST

    def exists(self, release_id: str) -> bool:
        try:
            return self.manifest_path(release_id).is_file()
        except StateError:
            return False

    def create(self, release: Release) -> None:
        directory = self.release_dir(release.release_id, create=True)
        path = directory / MANIFEST
        if path.exists():
            raise StateError(f"release already exists: {release.release_id}")
        self._write_json(path, release.data)
        self._write_json(self.root / ACTIVE, {"release_id": release.release_id})
        self.append_event(release, "release_planned", {"phase": release.data["phase"]})

    def save(self, release: Release) -> None:
        release.validate()
        self._write_json(self.manifest_path(release.release_id), release.data)

    def load(self, release_id: str | None = None) -> Release:
        chosen = release_id or self.active_release_id()
        path = self.manifest_path(chosen)
        return Release.from_json(self._read_json(path, maximum=2 * 1024 * 1024))

    def active_release_id(self) -> str:
        raw = self._read_json(self.root / ACTIVE, maximum=4096)
        if not isinstance(raw, dict) or set(raw) != {"release_id"}:
            raise StateError("active release pointer is malformed")
        return require_release_id(raw["release_id"])

    def append_event(self, release: Release, kind: str, detail: dict[str, Any]) -> None:
        directory = self.release_dir(release.release_id)
        path = directory / JOURNAL
        flags = os.O_APPEND | os.O_CREAT | os.O_WRONLY | getattr(os, "O_CLOEXEC", 0) | os.O_NOFOLLOW
        fd = os.open(path, flags, 0o600)
        try:
            _require_private_file(fd, JOURNAL)
            event = {
                "at": utc_now(),
                "seq": release.data["journal_seq"],
                "kind": kind,
                "detail": detail,
            }
            payload = json.dumps(event, sort_keys=True, separators=(",", ":")) + "\n"
            os.write(fd, payload.encode())
            os.fsync(fd)
        finally:
            os.close(fd)

    def checkpoint(self, release: Release, name: str, evidence: dict[str, Any]) -> bool:
        if not release.checkpoint(name, evidence):
            return False
        self.save(release)
        self.append_event(release, "checkpoint", {"name": name, "evidence": evidence})
        return True

    def _read_json(self, path: Path, *, maximum: int) -> Any:
        flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | os.O_NOFOLLOW
        try:
            fd = os.open(path, flags)
        except FileNotFoundError as error:
            raise StateError(f"required state file is missing: {path}") from error
        try:
            _require_private_file(fd, path.name)
            chunks = []
            remaining = maximum + 1
            while remaining:
                chunk = os.read(fd, min(65536, remaining))
                if not chunk:
                    break
                chunks.append(chunk)
                remaining -= len(chunk)
            payload = b"".join(chunks)
            if len(payload) > maximum:
                raise StateError(f"state file exceeds {maximum} bytes: {path}")
            return json.loads(payload.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise StateError(f"state file is not canonical JSON: {path}") from error
        finally:
            os.close(fd)

    def _write_json(self, path: Path, value: Any) -> None:
        directory = path.parent
        _require_private_dir(directory, create=False)
        payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
        temporary = directory / f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp"
        flags = os.O_CREAT | os.O_EXCL | os.O_WRONLY | getattr(os, "O_CLOEXEC", 0) | os.O_NOFOLLOW
        fd = os.open(temporary, flags, 0o600)
        try:
            _require_private_file(fd, temporary.name)
            os.write(fd, payload)
            os.fsync(fd)
        except BaseException:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass
            raise
        finally:
            os.close(fd)
        os.replace(temporary, path)
        dir_fd = os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
