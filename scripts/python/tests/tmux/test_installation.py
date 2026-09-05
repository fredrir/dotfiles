import json
import subprocess
import sys
from pathlib import Path

import pytest

from .harness import rust_binary


@pytest.fixture(scope="module")
def dotfile_binary():
    return rust_binary("dotfile-cli", "dotfile")


@pytest.mark.parametrize("mode", ["dry-run", "ready", "missing", "failed"])
def test_sync_provisions_plugins_only_when_needed(dotfile_binary, environment, tmp_path, mode):
    root = tmp_path / "repo"
    for folder in ["config", "environment/test", "shared/tmux"]:
        (root / folder).mkdir(parents=True)
    (root / "config/targets.dotfile").write_text(
        "shared/tmux/plugins.lock.json = ~/.config/sync-tmux/plugins.lock.json\n"
    )
    (root / "environment/test/manifest").write_text("shared\n")
    (root / "shared/tmux/plugins.lock.json").write_text("{}\n")
    log = tmp_path / "plugins.jsonl"
    binary = Path(environment["HOME"]) / ".local/bin/tmux-workspace"
    binary.parent.mkdir(parents=True)
    binary.write_text(
        f"#!{sys.executable}\n"
        "import json,sys\n"
        f"with open({str(log)!r}, 'a') as out: out.write(json.dumps(sys.argv[1:]) + '\\n')\n"
        "if 'status' in sys.argv:\n"
        f" print(json.dumps({{'resurrect': {{'installed': {mode == 'ready'}}}, 'fingers': {{'installed': {mode == 'ready'}}}}}))\n"
        f"elif {mode == 'failed'}:\n"
        " print('fixture download failed', file=sys.stderr)\n"
        " sys.exit(7)\n"
    )
    binary.chmod(0o700)
    result = subprocess.run(
        [
            dotfile_binary,
            "sync",
            "test",
            "--verbose",
            *(["--dry-run"] if mode == "dry-run" else []),
        ],
        env=environment | {"DOTFILE_ROOT": str(root), "DOTFILE_PYTHON": "/usr/bin/true"},
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
        timeout=20,
    )
    assert result.returncode == 0, (result.stdout, result.stderr)
    calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    base = ["--config", str(root / "shared/tmux"), "plugins"]
    if mode == "dry-run":
        assert calls == []
        assert not (Path(environment["HOME"]) / ".config/dotfile/sync/docs.fingerprint").exists()
    else:
        assert calls[0] == base + ["status", "--json"]
        assert calls[1:] == ([] if mode == "ready" else [base + ["install"]])
    if mode == "failed":
        assert "fixture download failed" in result.stdout + result.stderr
        assert "run tmux-workspace plugins install" in result.stdout + result.stderr
