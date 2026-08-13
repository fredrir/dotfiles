import json
import os
import shlex
import shutil
import subprocess
import tempfile

from tools.core.paths import repo_root
from tools.core.process import capture as run
from tools.utils.sysinfo.bench.record import HOST, LIB
from tools.utils.sysinfo.bench.suites import (
    Job,
    MeasurementError,
    Output,
    tool_path,
    version_of,
)

WARMUP = "2"
RUNS = "5"
PROBE_SECONDS = 20
MEASURE_SECONDS = 180


def candidates():
    root = str(repo_root())
    found = []
    if shutil.which("nvim"):
        found.append(("workload.nvim_startup", ["nvim", "--headless", "+qa"], root))
    if shutil.which("git"):
        found.append(("workload.git_status", ["git", "-C", root, "status", "--porcelain"], root))
        found.append(
            ("workload.git_log", ["git", "-C", root, "log", "--oneline", "-n", "200"], root)
        )
    if shutil.which("tar"):
        found.append(("workload.tar_repo", ["tar", "-cf", os.devnull, "scripts/src"], root))
    return found


def displayed(command, directory):
    # The repository root carries the username, and these records are committed
    # to a public repository. Record the command without it.
    return " ".join("." if part == directory else part for part in command)


def responsive(command, directory):
    try:
        result = run(command, cwd=directory, timeout=PROBE_SECONDS, stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        return False
    except OSError:
        return False
    return result.returncode == 0


def workloads():
    return [entry for entry in candidates() if responsive(entry[1], entry[2])]


def timings(path, command, directory):
    with tempfile.TemporaryDirectory(prefix="bench-workload-") as scratch:
        export = os.path.join(scratch, "result.json")
        result = run(
            [
                path,
                "-N",
                "--warmup",
                WARMUP,
                "--runs",
                RUNS,
                "--export-json",
                export,
                "--command-name",
                "workload",
                shlex.join(command),
            ],
            cwd=directory,
            timeout=MEASURE_SECONDS,
            stdin=subprocess.DEVNULL,
        )
        if result.returncode != 0:
            raise MeasurementError(f"hyperfine exited {result.returncode}")
        try:
            with open(export, encoding="utf-8") as handle:
                payload = json.load(handle)
        except (OSError, json.JSONDecodeError) as error:
            raise MeasurementError("hyperfine produced unreadable output") from error
    entries = payload.get("results") or []
    if not entries:
        raise MeasurementError("hyperfine reported no results")
    times = entries[0].get("times") or [entries[0]["median"]]
    return [float(value) * 1000 for value in times]


def jobs(setting):
    path = tool_path("hyperfine")
    if not path:
        return []
    version = version_of(path, args=("--version",), pattern=r"(\d[\d.]*)")
    found = []
    for key, command, directory in workloads():
        found.append(
            Job(
                name=key,
                tool="hyperfine",
                version=version,
                method=f"{key}/1.0.0",
                outputs=(Output(key, "ms", LIB, HOST),),
                measure=lambda key=key, command=command, directory=directory: {
                    key: timings(path, command, directory)
                },
                repeat=False,
                detail={
                    "command": displayed(command, directory),
                    "runs": RUNS,
                    "warmup": WARMUP,
                },
            )
        )
    return found
