from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any

from tools.dmux_rollout.errors import StateError

SCHEMA_VERSION = 1
RELEASE_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,79}$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SPACE_UID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


def utc_now() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds").replace("+00:00", "Z")


def require_release_id(value: str) -> str:
    if not RELEASE_RE.fullmatch(value):
        raise StateError("release id must be 1-80 lowercase ASCII letters, digits, '.', '_' or '-'")
    return value


def require_commit(value: str, field: str = "commit") -> str:
    if not SHA_RE.fullmatch(value):
        raise StateError(f"{field} is not an exact lowercase 40-character Git commit")
    return value


def require_space_uid(value: str, field: str = "space_uid") -> str:
    if not SPACE_UID_RE.fullmatch(value):
        raise StateError(f"{field} is not a canonical lowercase UUID")
    return value


@dataclass
class Release:
    data: dict[str, Any]

    @classmethod
    def create(
        cls,
        *,
        release_id: str,
        dotfiles: dict[str, Any],
        wezterm: dict[str, Any],
        smoke_name: str,
        archie_host: str,
    ) -> Release:
        now = utc_now()
        release = cls(
            {
                "schema": SCHEMA_VERSION,
                "release_id": require_release_id(release_id),
                "created_at": now,
                "updated_at": now,
                "journal_seq": 0,
                "phase": "planned",
                "frozen": {"dotfiles": dotfiles, "wezterm": wezterm},
                "hosts": {
                    "mac": {"name": "macie", "service_label": "com.fredrir.wezterm-mux"},
                    "archie": {"ssh": archie_host},
                },
                "artifacts": {},
                "checkpoints": {},
                "smoke": {
                    "name": smoke_name,
                    "space_uid": None,
                    "host_uid": None,
                    "backend": "wez",
                },
                "rollback": {"mac": {}, "archie": {}},
            }
        )
        release.validate()
        return release

    @classmethod
    def from_json(cls, raw: Any) -> Release:
        if not isinstance(raw, dict):
            raise StateError("release manifest must be a JSON object")
        release = cls(raw)
        release.validate()
        return release

    @property
    def release_id(self) -> str:
        return self.data["release_id"]

    @property
    def checkpoints(self) -> dict[str, Any]:
        return self.data["checkpoints"]

    def validate(self) -> None:
        required = {
            "schema",
            "release_id",
            "created_at",
            "updated_at",
            "journal_seq",
            "phase",
            "frozen",
            "hosts",
            "artifacts",
            "checkpoints",
            "smoke",
            "rollback",
        }
        if set(self.data) != required:
            missing = sorted(required - set(self.data))
            extra = sorted(set(self.data) - required)
            raise StateError(f"release manifest keys differ (missing={missing}, extra={extra})")
        if self.data["schema"] != SCHEMA_VERSION:
            raise StateError(f"unsupported release schema {self.data['schema']!r}")
        require_release_id(self.data["release_id"])
        if not isinstance(self.data["journal_seq"], int) or self.data["journal_seq"] < 0:
            raise StateError("journal_seq must be a non-negative integer")
        for key in ("artifacts", "checkpoints", "rollback", "hosts", "smoke"):
            if not isinstance(self.data[key], dict):
                raise StateError(f"{key} must be a JSON object")
        frozen = self.data["frozen"]
        if not isinstance(frozen, dict) or set(frozen) != {"dotfiles", "wezterm"}:
            raise StateError("frozen must contain exactly dotfiles and wezterm")
        for name, source in frozen.items():
            if not isinstance(source, dict):
                raise StateError(f"frozen.{name} must be an object")
            require_commit(source.get("commit", ""), f"frozen.{name}.commit")
            if not isinstance(source.get("repo"), str) or not source["repo"].startswith("/"):
                raise StateError(f"frozen.{name}.repo must be absolute")
            dirty = source.get("main_worktree_dirty")
            if not isinstance(dirty, list) or not all(isinstance(item, str) for item in dirty):
                raise StateError(f"frozen.{name}.main_worktree_dirty must be a string array")
        smoke = self.data["smoke"]
        if not isinstance(smoke.get("name"), str) or not smoke["name"]:
            raise StateError("smoke.name must be nonempty")
        if smoke.get("space_uid") is not None:
            require_space_uid(smoke["space_uid"])

    def completed(self, name: str) -> bool:
        return name in self.checkpoints

    def checkpoint(self, name: str, evidence: dict[str, Any] | None = None) -> bool:
        if self.completed(name):
            return False
        self.checkpoints[name] = {"at": utc_now(), "evidence": evidence or {}}
        self.data["journal_seq"] += 1
        self.data["updated_at"] = utc_now()
        return True

    def set_phase(self, phase: str) -> None:
        self.data["phase"] = phase
        self.data["updated_at"] = utc_now()

    def set_smoke_identity(self, *, space_uid: str, host_uid: str) -> None:
        require_space_uid(space_uid)
        require_space_uid(host_uid, "host_uid")
        old = self.data["smoke"].get("space_uid")
        if old is not None and old != space_uid:
            raise StateError(f"smoke Space changed identity: {old} != {space_uid}")
        self.data["smoke"]["space_uid"] = space_uid
        self.data["smoke"]["host_uid"] = host_uid
        self.data["updated_at"] = utc_now()
