from dataclasses import dataclass
from typing import Any, Literal

Severity = Literal["error", "warning"]


@dataclass(frozen=True)
class Snapshot:
    hardware: dict[str, str]
    modules: dict[str, Any]
    shell_display: str
    terminal_display: str
    de_display: str
    wm_display: str
    nvidia: tuple[dict[str, Any], ...]
    probe_errors: tuple[str, ...] = ()

    def result(self, kind, fallback=None):
        value = self.modules.get(kind)
        return fallback if value is None else value


@dataclass(frozen=True)
class Fact:
    label: str
    value: str


@dataclass(frozen=True)
class Component:
    kind: str
    label: str
    vendor: str
    model: str
    art_kind: str = ""
    identifiers: tuple[str, ...] = ()
    facts: tuple[Fact, ...] = ()
    compact: bool = True


@dataclass(frozen=True)
class SoftwareBadge:
    kind: str
    vendor: str
    label: str
    identifiers: tuple[str, ...] = ()


@dataclass(frozen=True)
class SystemView:
    platform: SoftwareBadge
    machine_type: str
    summary: tuple[str, ...]
    components: tuple[Component, ...]
    software: tuple[SoftwareBadge, ...]
    system_facts: tuple[Fact, ...]


@dataclass(frozen=True)
class HealthIssue:
    severity: Severity
    title: str
    detail: str = ""
    action: str = ""


@dataclass(frozen=True)
class RenderOptions:
    full: bool = False
    health: bool = False
