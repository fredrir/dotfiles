import hashlib
import statistics
from dataclasses import dataclass, field, replace

from tools.utils.sysinfo.formatting import as_dict, as_list

SCHEMA = 1

HIB = "HIB"
LIB = "LIB"

WORLD = "world"
PLATFORM = "platform"
HOST = "host"

CLEAN = "clean"
NOISY = "noisy"
ABORTED = "aborted"

TIERS = ("quick", "standard", "heavy")


@dataclass(frozen=True)
class Metric:
    key: str
    method: str
    scale: str
    proportion: str
    comparable: str
    tool: str = ""
    tool_version: str = ""
    samples: tuple[float, ...] = ()
    detail: dict = field(default_factory=dict)

    @property
    def times_to_run(self):
        return len(self.samples)

    @property
    def median(self):
        if not self.samples:
            return None
        return statistics.median(self.samples)

    @property
    def mad(self):
        if not self.samples:
            return None
        center = statistics.median(self.samples)
        return statistics.median([abs(value - center) for value in self.samples])

    @property
    def rsd_pct(self):
        return relative_deviation(self.samples)

    @property
    def family(self):
        return self.key.split(".", 1)[0]

    def better(self, other):
        if self.median is None or other is None:
            return None
        if self.proportion == LIB:
            return self.median < other
        return self.median > other

    def to_json(self):
        payload = {
            "key": self.key,
            "method": self.method,
            "tool": self.tool,
            "tool_version": self.tool_version,
            "scale": self.scale,
            "proportion": self.proportion,
            "comparable": self.comparable,
            "times_to_run": self.times_to_run,
            "samples": list(self.samples),
            "median": self.median,
            "mad": self.mad,
            "rsd_pct": self.rsd_pct,
        }
        if self.detail:
            payload["detail"] = self.detail
        return payload

    @staticmethod
    def from_json(payload):
        return Metric(
            key=payload.get("key", ""),
            method=payload.get("method", ""),
            scale=payload.get("scale", ""),
            proportion=payload.get("proportion", HIB),
            comparable=payload.get("comparable", HOST),
            tool=payload.get("tool", ""),
            tool_version=payload.get("tool_version", ""),
            samples=tuple(payload.get("samples") or ()),
            detail=as_dict(payload.get("detail")),
        )


@dataclass(frozen=True)
class Run:
    run_id: str
    host: str
    started: str
    tier: str
    grade: str
    snapshot: dict = field(default_factory=dict)
    install: dict = field(default_factory=dict)
    conditions: dict = field(default_factory=dict)
    metrics: tuple[Metric, ...] = ()
    note: str = ""
    tags: tuple[str, ...] = ()
    dotfiles_sha: str = ""
    gate_reasons: tuple[str, ...] = ()
    bytes_written: int = 0
    schema: int = SCHEMA

    @property
    def epoch(self):
        return epoch_of(self.snapshot)

    @property
    def os_id(self):
        return self.install.get("os", "")

    def metric(self, key):
        for entry in self.metrics:
            if entry.key == key:
                return entry
        return None

    def to_json(self):
        return {
            "schema": self.schema,
            "run_id": self.run_id,
            "host": self.host,
            "epoch": self.epoch,
            "started": self.started,
            "tier": self.tier,
            "grade": self.grade,
            "note": self.note,
            "tags": list(self.tags),
            "dotfiles_sha": self.dotfiles_sha,
            "gate_reasons": list(self.gate_reasons),
            "bytes_written": self.bytes_written,
            "snapshot": self.snapshot,
            "install": self.install,
            "conditions": self.conditions,
            "metrics": [metric.to_json() for metric in self.metrics],
        }

    @staticmethod
    def from_json(payload):
        return Run(
            run_id=payload.get("run_id", ""),
            host=payload.get("host", ""),
            started=payload.get("started", ""),
            tier=payload.get("tier", ""),
            grade=payload.get("grade", ""),
            snapshot=as_dict(payload.get("snapshot")),
            install=as_dict(payload.get("install")),
            conditions=as_dict(payload.get("conditions")),
            metrics=tuple(Metric.from_json(item) for item in as_list(payload.get("metrics"))),
            note=payload.get("note", ""),
            tags=tuple(payload.get("tags") or ()),
            dotfiles_sha=payload.get("dotfiles_sha", ""),
            gate_reasons=tuple(payload.get("gate_reasons") or ()),
            bytes_written=payload.get("bytes_written") or 0,
            schema=payload.get("schema") or SCHEMA,
        )


def relative_deviation(samples):
    values = [value for value in samples if isinstance(value, (int, float))]
    if len(values) < 2:
        return 0.0
    mean = statistics.fmean(values)
    if not mean:
        return 0.0
    return abs(statistics.stdev(values) / mean) * 100


def method_series(method):
    name, _, version = method.partition("/")
    parts = version.split(".")
    major = parts[0] if parts else ""
    minor = parts[1] if len(parts) > 1 else ""
    return f"{name}/{major}.{minor}"


def comparable_methods(left, right):
    return method_series(left.method) == method_series(right.method)


GIB = 1024**3


def whole_gib(value):
    """Capacity rounded down to GiB, as a string.

    Capacities arrive from several sources at several precisions -- VRAM comes
    from nvidia-smi as a float and from fastfetch as an int, and the two differ
    by a few hundred MiB for the same card. Comparing the raw values meant a
    three second nvidia-smi timeout silently changed the machine's identity and
    orphaned its pinned baseline, after which real regressions went unreported.
    """
    try:
        return str(int(float(value) // GIB))
    except (TypeError, ValueError):
        return ""


def identity_fields(snapshot):
    cpu = as_dict(snapshot.get("cpu"))
    memory = as_dict(snapshot.get("memory"))
    values = [
        cpu.get("model", ""),
        str(cpu.get("cores_physical") or ""),
        str(cpu.get("cores_logical") or ""),
        whole_gib(memory.get("total")),
    ]
    # Sorted, because device enumeration order is not identity: the same two
    # drives reported in the other order used to read as a different machine.
    # memory.modules is deliberately absent -- it reads 0 without root and 2
    # with, so one sudo run was enough to orphan the baseline.
    values.extend(
        sorted(
            f"{gpu.get('name', '')}:{whole_gib(gpu.get('memory_total'))}"
            for gpu in as_list(snapshot.get("gpu"))
        )
    )
    values.extend(
        sorted(
            f"{disk.get('name', '')}:{whole_gib(disk.get('size'))}"
            for disk in as_list(snapshot.get("disks"))
        )
    )
    return values


def epoch_of(snapshot):
    joined = "\n".join(identity_fields(snapshot))
    return hashlib.blake2s(joined.encode("utf-8"), digest_size=4).hexdigest()


def snapshot_differences(left, right):
    changes = []
    for label, key in (("CPU", "cpu"), ("Memory", "memory")):
        before, after = as_dict(left.get(key)), as_dict(right.get(key))
        for name in ("model", "total", "modules", "cores_physical"):
            if (name in before or name in after) and before.get(name) != after.get(name):
                changes.append((label, name, before.get(name), after.get(name)))
    for label, key, name in (("GPU", "gpu", "name"), ("Storage", "disks", "name")):
        before = [item.get(name, "") for item in as_list(left.get(key))]
        after = [item.get(name, "") for item in as_list(right.get(key))]
        if before != after:
            changes.append((label, name, ", ".join(before), ", ".join(after)))
    return tuple(changes)


def with_metrics(run, metrics):
    return replace(run, metrics=tuple(metrics))
