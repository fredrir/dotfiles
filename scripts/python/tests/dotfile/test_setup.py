import hashlib
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
RUST_BINARIES = [
    "agent-hop",
    "bench-workloads",
    "count",
    "doc-keybinds",
    "doc-purge",
    "dotfile",
    "dotfile-format",
    "dotfmt",
    "flatten",
    "gget",
    "git-discard",
    "gppf",
    "hpull",
    "hpush",
    "hwire",
    "mux-route",
    "path",
    "size",
    "sysinfo-collect",
]


def executable(path, body="#!/bin/sh\nexit 0\n"):
    path.write_text(body)
    path.chmod(0o755)


def digest(paths):
    value = hashlib.sha256()
    for path in paths:
        value.update(path.read_bytes())
    return value.hexdigest()


def rust_inputs():
    found = []
    for directory in (ROOT / "scripts/rust", ROOT / "shared/tools"):
        for parent, directories, files in os.walk(directory):
            directories[:] = [name for name in directories if name != "target"]
            found.extend(
                path
                for name in files
                if (path := Path(parent, name)).is_file() and not path.is_symlink()
            )
    return sorted(found, key=lambda path: os.fsencode(path))


def setup_environment(tmp_path):
    home = tmp_path / "home"
    binaries = home / ".local/bin"
    state = home / ".config/dotfile/sync"
    fake_path = tmp_path / "path"
    binaries.mkdir(parents=True)
    state.mkdir(parents=True)
    fake_path.mkdir()
    log = tmp_path / "dotfile.log"
    driver = '#!/bin/sh\nprintf \'%s\\n\' "$*" >> "$DOTFILE_TEST_LOG"\nexit 0\n'
    for name in RUST_BINARIES:
        executable(binaries / name, driver if name == "dotfile" else "#!/bin/sh\nexit 0\n")
    executable(binaries / "dotfile-py")
    for name in ("cargo", "git", "uv"):
        executable(fake_path / name)
    python_hash = digest([ROOT / "scripts/python/pyproject.toml", ROOT / "scripts/python/uv.lock"])
    (state / "python").write_text(f"{python_hash}\n")
    (state / "rust").write_text(f"{digest(rust_inputs())}\n")
    environment = dict(os.environ)
    environment.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / ".config"),
        XDG_DATA_HOME=str(home / ".local/share"),
        DOTFILE_TEST_LOG=str(log),
        PATH=f"{fake_path}:{environment['PATH']}",
    )
    return environment, log


def run_setup(tmp_path, *arguments):
    environment, log = setup_environment(tmp_path)
    result = subprocess.run(
        [ROOT / "setup.sh", *arguments],
        capture_output=True,
        text=True,
        env=environment,
        cwd=ROOT,
        check=False,
    )
    calls = log.read_text().splitlines()
    return result, calls


def setup_process(environment, *arguments):
    return subprocess.Popen(
        [ROOT / "setup.sh", *arguments],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
        cwd=ROOT,
    )


def wait_for(path, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.01)
    raise AssertionError(f"timed out waiting for {path}")


def test_setup_sync_forwards_native_sync_arguments(tmp_path):
    result, calls = run_setup(
        tmp_path,
        "--sync",
        "arch-linux/hyprland",
        "--",
        "--override",
        "linux/hyprland=none",
        "-n",
    )
    assert result.returncode == 0, result.stderr
    assert calls[-1] == "sync arch-linux/hyprland --override linux/hyprland=none -n"
    assert not any(call.startswith("link ") for call in calls)


def test_first_setup_uses_the_same_native_sync_engine(tmp_path):
    result, calls = run_setup(tmp_path, "--macos", "--", "-n")
    assert result.returncode == 0, result.stderr
    assert "secret init" in calls
    assert calls[-1] == "sync macos -n"
    assert not any(call.startswith("link ") or call == "secret doctor" for call in calls)


def test_concurrent_setups_serialize_installation(tmp_path):
    environment, _ = setup_environment(tmp_path)
    state = Path(environment["XDG_CONFIG_HOME"]) / "dotfile/sync"
    (state / "python").unlink()
    activity = tmp_path / "uv.activity"
    environment["DOTFILE_UV_ACTIVITY"] = str(activity)
    executable(
        tmp_path / "path/uv",
        "#!/bin/sh\n"
        'if [ "$1 $2" = "tool install" ]; then\n'
        "  printf 'begin\\n' >> \"$DOTFILE_UV_ACTIVITY\"\n"
        "  sleep 0.4\n"
        "  printf 'end\\n' >> \"$DOTFILE_UV_ACTIVITY\"\n"
        "fi\n"
        "exit 0\n",
    )

    first = setup_process(environment, "--commands-only")
    wait_for(activity)
    second = setup_process(environment, "--commands-only")
    first_stdout, first_stderr = first.communicate(timeout=10)
    second_stdout, second_stderr = second.communicate(timeout=10)

    assert first.returncode == 0, first_stderr
    assert second.returncode == 0, second_stderr
    assert activity.read_text().splitlines() == ["begin", "end"]
    assert "another setup is running; waiting" in second_stdout
    assert "workstation commands are current" in second_stdout
    assert not (state.parent / "setup.lock.d").exists()
    assert "installing workstation commands" in first_stdout


def test_setup_recovers_lock_owned_by_dead_process(tmp_path):
    environment, _ = setup_environment(tmp_path)
    lock = Path(environment["XDG_CONFIG_HOME"]) / "dotfile/setup.lock.d"
    lock.mkdir()
    (lock / "pid").write_text("2147483647\n")

    result = subprocess.run(
        [ROOT / "setup.sh", "--commands-only"],
        capture_output=True,
        text=True,
        env=environment,
        cwd=ROOT,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert not lock.exists()
    assert not list(lock.parent.glob("setup.lock.d.stale.*"))


def test_failed_staged_binary_validation_preserves_installed_tools(tmp_path):
    environment, _ = setup_environment(tmp_path)
    binaries = Path(environment["HOME"]) / ".local/bin"
    state = Path(environment["XDG_CONFIG_HOME"]) / "dotfile/sync"
    (state / "rust").write_text("outdated\n")
    before = {name: (binaries / name).read_bytes() for name in RUST_BINARIES}
    executable(
        tmp_path / "path/install",
        "#!/bin/sh\n"
        'for argument do destination="$argument"; done\n'
        'if [ "${destination##*/}" = dotfile ]; then\n'
        "  printf '#!/bin/sh\\nexit 71\\n' > \"$destination\"\n"
        "else\n"
        "  printf '#!/bin/sh\\nexit 0\\n' > \"$destination\"\n"
        "fi\n"
        'chmod 0755 "$destination"\n',
    )

    result = subprocess.run(
        [ROOT / "setup.sh", "--commands-only"],
        capture_output=True,
        text=True,
        env=environment,
        cwd=ROOT,
        check=False,
    )

    assert result.returncode == 71
    assert {name: (binaries / name).read_bytes() for name in RUST_BINARIES} == before
    assert (state / "rust").read_text() == "outdated\n"
    assert not list(binaries.glob(".dotfile-native.*"))
    assert not (state.parent / "setup.lock.d").exists()


def test_failed_native_rename_rolls_back_every_installed_tool(tmp_path):
    environment, _ = setup_environment(tmp_path)
    binaries = Path(environment["HOME"]) / ".local/bin"
    state = Path(environment["XDG_CONFIG_HOME"]) / "dotfile/sync"
    (state / "rust").write_text("outdated\n")
    for name in RUST_BINARIES:
        executable(binaries / name, f"#!/bin/sh\n# old-{name}\nexit 0\n")
    before = {name: (binaries / name).read_bytes() for name in RUST_BINARIES}
    executable(
        tmp_path / "path/install",
        "#!/bin/sh\n"
        'for argument do destination="$argument"; done\n'
        "name=${destination##*/}\n"
        'printf \'#!/bin/sh\\n# new-%s\\nexit 0\\n\' "$name" > "$destination"\n'
        'chmod 0755 "$destination"\n',
    )
    environment["DOTFILE_REAL_MV"] = shutil.which("mv") or "/bin/mv"
    environment["DOTFILE_MV_FAILED"] = str(tmp_path / "mv.failed")
    executable(
        tmp_path / "path/mv",
        "#!/bin/sh\n"
        "previous=\n"
        "for argument do source=$previous; previous=$argument; done\n"
        'case "$source" in\n'
        "*/.dotfile-native.*/doc-purge)\n"
        '  if [ ! -e "$DOTFILE_MV_FAILED" ]; then\n'
        '    : > "$DOTFILE_MV_FAILED"\n'
        "    exit 79\n"
        "  fi\n"
        "  ;;\n"
        "esac\n"
        'exec "$DOTFILE_REAL_MV" "$@"\n',
    )

    result = subprocess.run(
        [ROOT / "setup.sh", "--commands-only"],
        capture_output=True,
        text=True,
        env=environment,
        cwd=ROOT,
        check=False,
    )

    assert result.returncode == 1
    assert "could not install native tool 'doc-purge'" in result.stderr
    assert {name: (binaries / name).read_bytes() for name in RUST_BINARIES} == before
    assert (state / "rust").read_text() == "outdated\n"
    assert not list(binaries.glob(".dotfile-native.*"))
    assert not (state.parent / "setup.lock.d").exists()


def test_signal_during_native_commit_finishes_batch_then_returns_signal(tmp_path):
    environment, _ = setup_environment(tmp_path)
    binaries = Path(environment["HOME"]) / ".local/bin"
    state = Path(environment["XDG_CONFIG_HOME"]) / "dotfile/sync"
    (state / "rust").write_text("outdated\n")
    for name in RUST_BINARIES:
        executable(binaries / name, f"#!/bin/sh\n# old-{name}\nexit 0\n")
    executable(
        tmp_path / "path/install",
        "#!/bin/sh\n"
        'for argument do destination="$argument"; done\n'
        "name=${destination##*/}\n"
        'printf \'#!/bin/sh\\n# new-%s\\nexit 0\\n\' "$name" > "$destination"\n'
        'chmod 0755 "$destination"\n',
    )
    environment["DOTFILE_REAL_MV"] = shutil.which("mv") or "/bin/mv"
    environment["DOTFILE_MV_SIGNALED"] = str(tmp_path / "mv.signaled")
    executable(
        tmp_path / "path/mv",
        "#!/bin/sh\n"
        "previous=\n"
        "for argument do source=$previous; previous=$argument; done\n"
        '"$DOTFILE_REAL_MV" "$@" || exit $?\n'
        'case "$source" in\n'
        "*/.dotfile-native.*/bench-workloads)\n"
        '  if [ ! -e "$DOTFILE_MV_SIGNALED" ]; then\n'
        '    : > "$DOTFILE_MV_SIGNALED"\n'
        "    kill -TERM 0\n"
        "  fi\n"
        "  ;;\n"
        "esac\n",
    )

    result = subprocess.run(
        [ROOT / "setup.sh", "--commands-only"],
        capture_output=True,
        text=True,
        env=environment,
        cwd=ROOT,
        check=False,
        start_new_session=True,
    )

    assert result.returncode == 128 + signal.SIGTERM
    assert all(f"# new-{name}\n" in (binaries / name).read_text() for name in RUST_BINARIES)
    assert (state / "rust").read_text().strip() == digest(rust_inputs())
    assert not list(binaries.glob(".dotfile-native.*"))
    assert not (state.parent / "setup.lock.d").exists()
