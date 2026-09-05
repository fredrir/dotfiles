import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from .harness import SOURCE, wait_for


def lockfile(environment):
    path = Path(environment["DOTFILES_TMUX_CONFIG"]) / "plugins.lock.json"
    return path, json.loads(path.read_text())


def artifact_paths(environment, lock):
    root = Path(environment["DOTFILES_TMUX_PLUGIN_HOME"])
    return root / ("resurrect-" + lock["resurrect"]["revision"]), root / (
        "fingers-" + lock["fingers"]["version"]
    ) / "tmux-fingers"


def executable(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    path.chmod(0o700)
    return path


@pytest.fixture
def fake_plugins(environment):
    _, lock = lockfile(environment)
    resurrect, fingers = artifact_paths(environment, lock)
    for action in ("save", "restore"):
        executable(resurrect / f"scripts/{action}.sh", "#!/bin/sh\nexit 0\n")
    executable(
        fingers,
        f'#!/bin/sh\ncase "$1" in version) echo {lock["fingers"]["version"]};; load-config) exit 0;; *) exit 0;; esac\n',
    )
    return resurrect, fingers


def test_offline_install_is_server_independent(invoke, environment, fake_plugins, tmp_path):
    invoke("plugins", "install", env={"TMUX_BINARY": "/nonexistent/tmux"})
    assert (Path(environment["DOTFILES_TMUX_PLUGIN_HOME"]) / "installation.json").is_file()


def test_reload_never_downloads_missing_plugins(server, environment):
    called = Path(environment["HOME"]) / "network"
    for program in ("git", "curl"):
        executable(
            Path(environment["PATH"].split(os.pathsep)[0]) / program,
            f"#!/bin/sh\ntouch '{called}'\nexit 1\n",
        )
    server.load()
    wait_for(lambda: server.tm("show-options", "-gqv", "@workspace-plugins-state") == "error")
    assert not called.exists()
    assert not Path(environment["DOTFILES_TMUX_PLUGIN_HOME"]).exists()


def test_load_failure_is_server_local(server, environment, fake_plugins):
    server.load()
    server.run("plugins", "load")
    _, fingers = fake_plugins
    executable(
        fingers,
        '#!/bin/sh\ncase "$1" in version) echo 2.7.1;; *) echo broken-style >&2; exit 1;; esac\n',
    )
    result = server.run("plugins", "load", check=False)
    assert "broken-style" in result.stderr
    assert server.tm("show-options", "-gqv", "@workspace-plugins-state") == "error"
    assert not (Path(environment["DOTFILES_TMUX_PLUGIN_HOME"]) / "installation.json").exists()
    executable(fingers, '#!/bin/sh\ncase "$1" in version) echo 2.7.1;; *) exit 0;; esac\n')
    server.run("plugins", "load")
    assert server.tm("show-options", "-gqv", "@workspace-plugins-state") == ""
    assert server.tm("show-options", "-gqv", "@workspace-plugins-error") == ""


def test_system_fingers_version_is_verified_and_reported(invoke, environment):
    fake = Path(environment["PATH"].split(os.pathsep)[0]) / "tmux-fingers"
    executable(fake, "#!/bin/sh\necho 0.0.0\n")
    status = json.loads(invoke("plugins", "status", "--json").stdout)
    assert not status["fingers"]["installed"]
    assert str(fake) in status["fingers"]["error"]
    assert "0.0.0" in status["fingers"]["error"]


def test_validation_has_no_install_or_filesystem_effects(invoke, environment):
    invoke("plugins", "install", env={"DOTFILES_TMUX_VALIDATE": "1", "TMUX_BINARY": "/nonexistent"})
    invoke("plugins", "load", env={"DOTFILES_TMUX_VALIDATE": "1", "TMUX_BINARY": "/nonexistent"})
    assert not Path(environment["DOTFILES_TMUX_PLUGIN_HOME"]).exists()


def test_float_and_multiple_clients_fall_back_without_launch(
    server, fake_plugins, picker, tmp_path
):
    _, fingers = fake_plugins
    log = tmp_path / "fingers-started"
    executable(
        fingers, f"#!/bin/sh\nif [ \"$1\" = version ]; then echo 2.7.1; else touch '{log}'; fi\n"
    )
    first = server.attach()
    server.attach()
    server.run("quick-select", client=first.name)
    assert not log.exists()
    server.run("scratch")
    pane = next(p["id"] for p in server.panes() if p["tool"] == "scratch-view")
    assert server.run("plugins", "fingers", pane=pane, check=False).returncode == 3
    assert not log.exists()


@pytest.fixture
def download_fixture(environment, tmp_path):
    path, lock = lockfile(environment)
    repository = tmp_path / "resurrect-source"
    for action in ("save", "restore"):
        executable(repository / f"scripts/{action}.sh", "#!/bin/sh\nexit 0\n")
    subprocess.run(["git", "init", "-q", str(repository)], check=True)
    subprocess.run(["git", "-C", str(repository), "add", "."], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repository),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
        check=True,
    )
    revision = subprocess.check_output(
        ["git", "-C", str(repository), "rev-parse", "HEAD"], text=True
    ).strip()
    lock["resurrect"] = {"repository": str(repository), "revision": revision}
    payload = (
        f'#!/bin/sh\nif [ "$1" = version ]; then echo {lock["fingers"]["version"]}; fi\n'.encode()
    )
    payload_file = tmp_path / "binary"
    payload_file.write_bytes(payload)
    digest = hashlib.sha256(payload).hexdigest()
    for asset in lock["fingers"]["assets"].values():
        asset["sha256"] = digest
    path.write_text(json.dumps(lock))
    curl = Path(environment["PATH"].split(os.pathsep)[0]) / "curl"
    executable(
        curl,
        f"#!{sys.executable}\nimport pathlib,sys\nargs=sys.argv[1:]\npathlib.Path(args[args.index('--output')+1]).write_bytes(pathlib.Path({str(payload_file)!r}).read_bytes())\n",
    )
    return lock, payload_file


def test_verified_install_is_atomic_and_pinned(invoke, environment, download_fixture):
    lock, _ = download_fixture
    invoke("plugins", "install", env={"DOTFILES_TMUX_OFFLINE": "0"})
    resurrect, fingers = artifact_paths(environment, lock)
    assert (
        subprocess.check_output(
            ["git", "-C", str(resurrect), "rev-parse", "HEAD"], text=True
        ).strip()
        == lock["resurrect"]["revision"]
    )
    assert fingers.stat().st_mode & 0o777 == 0o700
    assert (
        hashlib.sha256(fingers.read_bytes()).hexdigest()
        == next(iter(lock["fingers"]["assets"].values()))["sha256"]
    )


def test_corrupt_download_is_never_executed_or_published(
    invoke, environment, download_fixture, tmp_path
):
    lock, payload = download_fixture
    touched = tmp_path / "executed"
    payload.write_text(f"#!/bin/sh\ntouch '{touched}'\n")
    result = invoke("plugins", "install", env={"DOTFILES_TMUX_OFFLINE": "0"}, check=False)
    assert "checksum mismatch" in result.stderr
    assert not touched.exists()
    assert not artifact_paths(environment, lock)[1].parent.exists()
    assert not list(Path(environment["DOTFILES_TMUX_PLUGIN_HOME"]).glob(".fingers-*"))


def test_failed_download_can_be_retried(invoke, environment, download_fixture):
    lock, payload = download_fixture
    good = payload.read_bytes()
    payload.write_bytes(b"partial")
    assert (
        invoke("plugins", "install", env={"DOTFILES_TMUX_OFFLINE": "0"}, check=False).returncode
        == 1
    )
    payload.write_bytes(good)
    invoke("plugins", "install", env={"DOTFILES_TMUX_OFFLINE": "0"})
    assert artifact_paths(environment, lock)[1].is_file()


def test_missing_snapshot_never_launches_restore(server, fake_plugins, tmp_path):
    resurrect, _ = fake_plugins
    touched = tmp_path / "restore"
    executable(resurrect / "scripts/restore.sh", f"#!/bin/sh\ntouch '{touched}'\n")
    result = server.run("restore", "--yes", check=False)
    assert "no saved workspace" in result.stderr
    assert not touched.exists()


@pytest.fixture
def real_resurrect(environment):
    _, lock = lockfile(environment)
    original = json.loads((SOURCE / "plugins.lock.json").read_text())
    source = Path(
        os.environ.get(
            "TMUX_RESURRECT_SOURCE",
            str(
                Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share")))
                / "tmux/plugins"
                / ("resurrect-" + original["resurrect"]["revision"])
            ),
        )
    )
    if not source.is_dir():
        pytest.skip(
            "install pinned resurrect or set TMUX_RESURRECT_SOURCE for recovery integration"
        )
    actual = subprocess.check_output(
        ["git", "-C", str(source), "rev-parse", "HEAD"], text=True
    ).strip()
    assert actual == original["resurrect"]["revision"]
    destination, _ = artifact_paths(environment, lock)
    shutil.copytree(source, destination, symlinks=True)
    return destination


def test_real_save_restart_restore_preserves_metadata_and_existing_panes(server, real_resurrect):
    server.load()
    server.tm("set-option", "-g", "@resurrect-capture-pane-contents", "off")
    server.tm("set-option", "-t", "origin", "@workspace-root", server.env["HOME"])
    server.run("scratch")
    server.run("shelf-park")
    # Scratch views are transient; backing sessions and shelf metadata must survive.
    server.run("save", timeout=40)
    savedir = Path(server.tm("show-options", "-gqv", "@resurrect-dir"))
    saved = (savedir / "last").resolve()
    metadata = json.loads(saved.with_name(saved.name + ".workspace.json").read_text())
    assert metadata["views"]
    assert any(options.get("@workspace-tool") == "shelf" for options in metadata["panes"].values())
    server.stop()
    server.start()
    server.tm("rename-session", "-t", "origin", "existing")
    server.tm("set-option", "-g", "@resurrect-dir", str(savedir))
    existing_pid = server.fmt("#{pane_pid}")
    server.run("restore", "--yes", timeout=40)
    assert server.fmt("#{pane_pid}") == existing_pid
    restored = server.panes()
    shelf = next(p for p in restored if p["tool"] == "shelf")
    assert shelf["session_name"] == "__workspace-shelf"
    assert any(p["session_name"].startswith("__workspace-scratch-") for p in restored)
    assert not any(p["tool"] == "scratch-view" for p in restored)
    projects = server.run("projects", "--json").stdout
    assert "__workspace-" not in projects
    assert server.tm("show-options", "-qv", "-t", "origin", "@workspace-root") == server.env["HOME"]
    server.run("shelf", "--take", shelf["id"])
    assert next(p for p in server.panes() if p["id"] == shelf["id"])["session_name"] == "existing"


def test_restore_rejects_multiple_clients(server, real_resurrect):
    server.tm("set-option", "-g", "@resurrect-capture-pane-contents", "off")
    server.run("save", timeout=40)
    server.attach()
    server.attach()
    result = server.run("restore", "--yes", check=False)
    assert "single attached client" in result.stderr
