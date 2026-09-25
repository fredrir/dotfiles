"""setup.sh is a bootstrap: build `dotfile`, install it, hand over to `dotfile sync`.

Everything it used to do itself now lives in sync, covered by
scripts/rust/crates/dotfile/tests/tooling_install.rs and sync_selection.rs.
"""

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
SETUP = ROOT / "setup.sh"

DOTFILE_STUB = """#!/bin/sh
printf '%s\\n' "$*" >> "$DOTFILE_TEST_LOG"
exit 0
"""

CARGO_STUB = """#!/bin/sh
printf 'cargo %s\\n' "$*" >> "$DOTFILE_TEST_LOG"
exit 0
"""

# setup.sh runs under `/usr/bin/env bash` and shells out to these. They are
# linked into the sandbox PATH so that PATH holds nothing but the sandbox: a
# real cargo anywhere else on this machine must never stand in for the missing
# one the last test removes.
UTILITIES = ("bash", "mkdir", "cmp", "install", "mktemp", "mv", "rm")


def executable(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o755)


def link_utilities(path):
    search = os.environ.get("PATH", os.defpath)
    for name in UTILITIES:
        found = next(
            (
                os.path.join(entry, name)
                for entry in search.split(os.pathsep)
                if os.path.isfile(os.path.join(entry, name))
            ),
            None,
        )
        assert found is not None, f"{name} is not on PATH"
        (path / name).symlink_to(found)


def repository(tmp_path, *, cargo: str = CARGO_STUB, built: str | None = DOTFILE_STUB):
    """A repository whose `cargo` has already produced the installed binary."""
    root = tmp_path / "dotfiles"
    log = tmp_path / "dotfile.log"
    path = tmp_path / "path"
    path.mkdir(parents=True, exist_ok=True)
    executable(path / "cargo", cargo)
    link_utilities(path)
    if built is not None:
        executable(root / "scripts/rust/target/commands/dotfile", built)
    environment = dict(os.environ)
    environment.update(
        DOTFILE_ROOT=str(root),
        DOTFILE_TEST_LOG=str(log),
        PATH=str(path),
    )
    return root, environment, log


def run_setup(environment, *arguments):
    return subprocess.run(
        [SETUP, *arguments],
        capture_output=True,
        text=True,
        env=environment,
        cwd=ROOT,
        check=False,
    )


def calls(log):
    return log.read_text().splitlines() if log.exists() else []


def test_setup_builds_installs_then_hands_over_to_dotfile_sync(tmp_path):
    root, environment, log = repository(tmp_path)

    result = run_setup(environment)

    assert result.returncode == 0, result.stderr
    manifest = root / "scripts/rust/Cargo.toml"
    build = (
        f"cargo build --profile commands --locked --quiet --manifest-path {manifest} --bin dotfile"
    )
    assert calls(log) == [build, "sync"]
    assert os.access(root / ".bin/dotfile", os.X_OK)


def test_every_argument_reaches_dotfile_sync_verbatim(tmp_path):
    _, environment, log = repository(tmp_path)

    result = run_setup(
        environment, "arch-linux/hyprland", "--override", "linux/hyprland=none", "-n"
    )

    assert result.returncode == 0, result.stderr
    assert calls(log)[-1] == ("sync arch-linux/hyprland --override linux/hyprland=none -n")


def test_the_bootstrap_does_not_interpret_arguments(tmp_path):
    _, environment, log = repository(tmp_path)

    result = run_setup(environment, "--macos", "-n")

    assert result.returncode == 0, result.stderr
    assert calls(log)[-1] == "sync --macos -n"


def test_a_missing_binary_directory_is_created(tmp_path):
    root, environment, _ = repository(tmp_path)
    assert not (root / ".bin").exists()

    result = run_setup(environment)

    assert result.returncode == 0, result.stderr
    assert (root / ".bin/dotfile").is_file()


def test_an_unchanged_binary_is_not_reinstalled(tmp_path):
    root, environment, _ = repository(tmp_path)
    assert run_setup(environment).returncode == 0
    installed = root / ".bin/dotfile"
    os.utime(installed, (0, 0))

    assert run_setup(environment).returncode == 0

    assert installed.stat().st_mtime == 0, "an identical binary keeps its mtime"


def test_a_rebuilt_binary_replaces_the_installed_one(tmp_path):
    root, environment, _ = repository(tmp_path)
    assert run_setup(environment).returncode == 0
    executable(
        root / "scripts/rust/target/commands/dotfile",
        DOTFILE_STUB.replace("exit 0", "exit 0 # rebuilt"),
    )

    assert run_setup(environment).returncode == 0

    assert "rebuilt" in (root / ".bin/dotfile").read_text()


def test_a_failed_build_installs_nothing(tmp_path):
    root, environment, _ = repository(
        tmp_path,
        cargo="#!/bin/sh\necho 'error: could not compile' >&2\nexit 101\n",
        built=None,
    )

    result = run_setup(environment)

    assert result.returncode != 0
    assert "could not compile" in result.stderr
    assert not (root / ".bin/dotfile").exists()


def test_a_failed_build_keeps_the_installed_binary(tmp_path):
    root, environment, _ = repository(tmp_path)
    assert run_setup(environment).returncode == 0
    executable(tmp_path / "path/cargo", "#!/bin/sh\nexit 101\n")

    assert run_setup(environment).returncode != 0

    assert os.access(root / ".bin/dotfile", os.X_OK)


def test_a_missing_cargo_reports_how_to_get_one(tmp_path):
    _, environment, _ = repository(tmp_path)
    (tmp_path / "path/cargo").unlink()

    result = run_setup(environment)

    assert result.returncode == 1
    assert "cargo is required" in result.stderr
    assert "rustup.rs" in result.stderr


def test_no_staging_files_are_left_behind(tmp_path):
    root, environment, _ = repository(tmp_path)

    assert run_setup(environment).returncode == 0

    assert not list((root / ".bin").glob(".dotfile.*"))
