import hashlib
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

import pytest
from native import ROOT

pytestmark = pytest.mark.skipif(
    not os.environ.get("DOTFILE_PERF_OUTPUT"), reason="set DOTFILE_PERF_OUTPUT to record timings"
)

BASELINE = """
import sys, subprocess
from pathlib import Path
sys.path.insert(0, sys.argv.pop(1))
from tools.core import paths
paths.repo_root = lambda: Path(sys.argv.pop(1))
root = paths.repo_root()
paths.repo_root = lambda: root
from tools.utils.sysinfo import app
original = subprocess.Popen
count = 0
def popen(*args, **kwargs):
    global count
    count += 1
    return original(*args, **kwargs)
subprocess.Popen = popen
sys.argv[0] = 'sysinfo'
try:
    app()
finally:
    print(f'subprocess probes: {count}', file=sys.stderr)
"""


def measure(command, environment):
    if sys.platform == "darwin":
        timing = ["/usr/bin/time", "-l"]
        pattern, multiplier = r"(\d+)\s+maximum resident set size", 1
    else:
        timing = ["/usr/bin/time", "-f", "RSS_KIB:%M"]
        pattern, multiplier = r"RSS_KIB:(\d+)", 1024
    started = time.perf_counter()
    result = subprocess.run(
        [*timing, *command], env=environment, capture_output=True, check=False, timeout=45
    )
    elapsed = time.perf_counter() - started
    stderr = result.stderr.decode(errors="replace")
    memory = re.search(pattern, stderr)
    probes = re.search(r"subprocess probes: (\d+)", stderr)
    assert result.returncode == 0, f"{command}: {stderr}"
    return {
        "elapsed_ms": elapsed * 1000,
        "peak_rss_bytes": int(memory[1]) * multiplier if memory else None,
        "reported_subprocess_probes": int(probes[1]) if probes else None,
        "stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
    }


def test_record_cli_performance():
    baseline = os.environ.get("DOTFILE_PERF_BASELINE_SOURCE")
    executable = os.environ.get("DOTFILE_PERF_SYSINFO")
    assert baseline or executable, "set baseline source or release sysinfo executable"
    prefix = (
        [sys.executable, "-c", BASELINE, baseline, str(ROOT)] if baseline else [executable]
    )
    environment = dict(os.environ, NO_COLOR="1", COLUMNS="120", DOTFILE_ROOT=str(ROOT))
    repeats = int(os.environ.get("DOTFILE_PERF_REPEATS", "5"))
    cases = {
        "help": ["--help"],
        "summary": [],
        "pretty": ["--pretty"],
        "full": ["--full"],
        "history": ["bench", "list"],
        "completion": ["__complete", "runs"],
    }
    docs = os.environ.get("DOTFILE_PERF_DOTFILE")
    if docs:
        cases.update({"docs": ["docs", "--dry-run"], "keybinds": ["docs", "--only", "keybinds", "--dry-run"]})
    results = {}
    for name, arguments in cases.items():
        command = [docs, *arguments] if name in {"docs", "keybinds"} else [*prefix, *arguments]
        if not baseline and name in {"summary", "pretty", "full"}:
            command.append("--timings")
        first = measure(command, environment)
        samples = [measure(command, environment) for _ in range(repeats)]
        results[name] = {
            "first_ms": first["elapsed_ms"],
            "warm_median_ms": statistics.median(s["elapsed_ms"] for s in samples),
            "warm_max_ms": max(s["elapsed_ms"] for s in samples),
            "peak_rss_bytes": max(s["peak_rss_bytes"] or 0 for s in samples),
            "reported_subprocess_probes": samples[-1]["reported_subprocess_probes"],
            "samples": samples,
        }
    report = {
        "version": 1,
        "platform": platform.platform(),
        "implementation": "python-baseline" if baseline else "rust-release",
        "repeats": repeats,
        "cases": results,
    }
    Path(os.environ["DOTFILE_PERF_OUTPUT"]).write_text(json.dumps(report, indent=2) + "\n")
