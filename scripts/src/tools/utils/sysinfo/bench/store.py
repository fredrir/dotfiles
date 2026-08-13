import json
import os
from contextlib import contextmanager
from pathlib import Path

from tools.core import blocks
from tools.core.paths import repo_root
from tools.utils.sysinfo.bench.record import Run

BASELINES = "baselines.dotfile"
DOCUMENT = "BENCHMARKS.md"
LOCK = ".lock"

STRUCTURE_ERRORS = {
    blocks.UNEXPECTED_CLOSE: "unexpected }",
    blocks.NESTED: "nested host",
    blocks.OUTSIDE: "entry outside a host",
}


class LockedError(Exception):
    pass


def benchmarks_dir():
    override = os.environ.get("SYSINFO_BENCHMARKS")
    if override:
        return Path(override)
    return repo_root() / "benchmarks"


def host_dir(host):
    return benchmarks_dir() / host


def run_path(host, run_id):
    return host_dir(host) / f"{run_id}.json"


def save_run(run):
    path = run_path(run.host, run.run_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".json.partial")
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(run.to_json(), handle, indent=2, ensure_ascii=False, sort_keys=False)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
    return path


def load_run(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return Run.from_json(json.load(handle))
    except (OSError, json.JSONDecodeError):
        return None


def known_hosts():
    root = benchmarks_dir()
    if not root.is_dir():
        return []
    return sorted(entry.name for entry in root.iterdir() if entry.is_dir())


def list_runs(host=None, grades=None):
    hosts = [host] if host else known_hosts()
    found = []
    for name in hosts:
        directory = host_dir(name)
        if not directory.is_dir():
            continue
        for path in sorted(directory.glob("*.json")):
            run = load_run(path)
            if run is None:
                continue
            if grades and run.grade not in grades:
                continue
            found.append(run)
    return sorted(found, key=lambda run: run.started, reverse=True)


def baselines_path():
    return benchmarks_dir() / BASELINES


def load_baselines():
    path = baselines_path()
    if not path.is_file():
        return {}
    entries = blocks.read(str(path))
    found = {}
    for entry in entries:
        if entry.opens:
            found.setdefault(entry.block, {})
            continue
        epoch, run_id = entry.split("=")
        if epoch and run_id:
            found[entry.block][epoch] = run_id
    return found


def render_baselines(baselines):
    parts = []
    for host in sorted(baselines):
        pinned = baselines[host]
        if not pinned:
            continue
        if parts:
            parts.append("\n")
        parts.append(f"{host} {{\n")
        for epoch in sorted(pinned):
            parts.append(f"  {epoch} = {pinned[epoch]}\n")
        parts.append("}\n")
    return "".join(parts)


def save_baselines(baselines):
    path = baselines_path()
    content = render_baselines(baselines)
    if not content:
        if path.is_file():
            path.unlink()
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(content)
    return path


def set_baseline(host, epoch, run_id):
    baselines = load_baselines()
    baselines.setdefault(host, {})[epoch] = run_id
    save_baselines(baselines)


def clear_baseline(host, epoch):
    baselines = load_baselines()
    if host in baselines and epoch in baselines[host]:
        del baselines[host][epoch]
        if not baselines[host]:
            del baselines[host]
        save_baselines(baselines)
        return True
    return False


def baseline_run(host, epoch):
    run_id = load_baselines().get(host, {}).get(epoch, "")
    if not run_id:
        return None
    return load_run(run_path(host, run_id))


def holder_alive(path):
    try:
        with open(path, encoding="utf-8") as handle:
            pid = int(handle.read().strip())
    except (OSError, ValueError):
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


@contextmanager
def exclusive():
    path = benchmarks_dir() / LOCK
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        handle = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    except FileExistsError as error:
        if holder_alive(path):
            raise LockedError(str(path)) from error
        path.unlink(missing_ok=True)
        try:
            handle = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError as second:
            raise LockedError(str(path)) from second
    try:
        os.write(handle, f"{os.getpid()}\n".encode())
        os.close(handle)
        yield
    finally:
        path.unlink(missing_ok=True)


def total_bytes_written(host=None):
    return sum(run.bytes_written for run in list_runs(host))


def prunable(host=None, keep=12):
    baselines = load_baselines()
    protected = set()
    for name, pinned in baselines.items():
        for run_id in pinned.values():
            protected.add((name, run_id))
    dropped = []
    for name in ([host] if host else known_hosts()):
        by_epoch = {}
        for run in list_runs(name):
            by_epoch.setdefault(run.epoch, []).append(run)
        for runs in by_epoch.values():
            ordered = sorted(runs, key=lambda run: run.started, reverse=True)
            for run in ordered[keep:]:
                if (run.host, run.run_id) in protected:
                    continue
                if run is ordered[-1]:
                    continue
                dropped.append(run)
    return dropped
