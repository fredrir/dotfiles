import fcntl
import json
import os
from contextlib import contextmanager
from pathlib import Path

from tools.core import blocks
from tools.core.console import die
from tools.core.dotfmt import formatted
from tools.core.paths import repo_root
from tools.utils.sysinfo.bench.record import Run

PROG = "sysinfo bench"

BASELINES = "baselines.dotfile"
DOCUMENT = "BENCHMARKS.md"
LOCK = ".lock"


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
    # ValueError covers JSONDecodeError and UnicodeDecodeError; AttributeError
    # and TypeError cover a file holding a JSON scalar rather than an object.
    # One corrupt byte in one run used to take down list, show, compare, trend,
    # health, prune and dotfile doctor alike.
    try:
        with open(path, encoding="utf-8") as handle:
            return Run.from_json(json.load(handle))
    except (OSError, ValueError, AttributeError, TypeError):
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
    try:
        entries = blocks.read(str(path))
    except blocks.BlockError as error:
        die(PROG, blocks.describe(error, BASELINES, "host"))
        return {}
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
    # `dotfmt` owns the `=` column, so this only has to emit the pins.
    return formatted("".join(parts), baselines_path())


def save_baselines(baselines):
    path = baselines_path()
    content = render_baselines(baselines)
    if not content:
        if path.is_file():
            path.unlink()
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    # Written the way save_run writes: this is a read-modify-write of every
    # host's pins, so a crash mid-truncate lost every baseline on the machine.
    temporary = path.with_suffix(".dotfile.partial")
    with open(temporary, "w", encoding="utf-8") as handle:
        handle.write(content)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
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


def holder_pid(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read().strip()
    except OSError:
        return ""


@contextmanager
def exclusive():
    """Hold the benchmark lock for the duration of the block.

    flock rather than an O_EXCL pid file, because the pid file admitted two
    holders two ways: a stale lock was unlinked by both racers, and the pid was
    written after the create, so a reader catching that window judged a live
    lock stale and stole it. The kernel arbitrates this correctly; the pid in
    the file is now only there to name the holder. The lock file is left in
    place on release -- unlinking it would let two processes lock two inodes.
    """
    path = benchmarks_dir() / LOCK
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = os.open(path, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise LockedError(holder_pid(path) or str(path)) from error
        os.ftruncate(handle, 0)
        os.write(handle, f"{os.getpid()}\n".encode())
        yield
    finally:
        os.close(handle)


def total_bytes_written(host=None):
    return sum(run.bytes_written for run in list_runs(host))


def prunable(host=None, keep=12):
    baselines = load_baselines()
    protected = set()
    for name, pinned in baselines.items():
        for run_id in pinned.values():
            protected.add((name, run_id))
    dropped = []
    for name in [host] if host else known_hosts():
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
