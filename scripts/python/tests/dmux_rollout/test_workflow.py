from pathlib import Path

import pytest

from tools.dmux_rollout.command import Result, Runner, remote_argv
from tools.dmux_rollout.errors import CommandError, Refusal
from tools.dmux_rollout.storage import RolloutStore
from tools.dmux_rollout.workflow import AMBIENT_MUX_VARS, Workflow, WorkflowConfig

from .helpers import git, pushed_repo, release


def config(tmp_path, dotfiles, wezterm):
    return WorkflowConfig(
        dotfiles_repo=dotfiles,
        wezterm_repo=wezterm,
        packages_root=tmp_path / "packages",
        mac_app=tmp_path / "WezTerm.app",
        mac_dmux=tmp_path / "bin/dmux",
        mac_pane_bootstrap=tmp_path / "bin/pane-bootstrap",
    )


def test_plan_freezes_pushed_commits_but_not_the_mutable_dirty_listing(tmp_path):
    dotfiles = pushed_repo(tmp_path, "dotfiles")
    wezterm = pushed_repo(tmp_path, "wezterm")
    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(store, Runner(), config(tmp_path, dotfiles, wezterm))

    with store.exclusive():
        first = workflow.plan(release_id="release-one", smoke_name="smoke")
    (dotfiles / "source.txt").write_text("dirty later\n", encoding="utf-8")
    with store.exclusive():
        second = workflow.plan(release_id="release-one", smoke_name="smoke")

    assert first.data == second.data
    assert first.data["frozen"]["dotfiles"]["commit"] == git(dotfiles, "rev-parse", "HEAD")


def test_plan_refuses_an_unpushed_commit(tmp_path):
    dotfiles = pushed_repo(tmp_path, "dotfiles")
    wezterm = pushed_repo(tmp_path, "wezterm")
    (wezterm / "source.txt").write_text("unpublished\n", encoding="utf-8")
    git(wezterm, "add", "source.txt")
    git(wezterm, "commit", "-m", "not pushed")
    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(store, Runner(), config(tmp_path, dotfiles, wezterm))

    with store.exclusive(), pytest.raises(Refusal, match="not contained in a remote branch"):
        workflow.plan(release_id="release-one", smoke_name="smoke")


def test_native_inventory_refuses_unapproved_and_unmanaged_panes(tmp_path):
    descriptor = {
        "epoch": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "sentinel_window_id": 0,
        "sentinel_tab_id": 0,
        "sentinel_pane_id": 0,
    }
    space = "11111111-1111-4111-8111-111111111111"
    rows = [
        {
            "window_id": 0,
            "tab_id": 0,
            "pane_id": 0,
            "workspace": f"dmux:system:{descriptor['epoch']}",
        },
        {
            "window_id": 1,
            "tab_id": 1,
            "pane_id": 7,
            "workspace": f"dmux:22222222-2222-4222-8222-222222222222:{space}",
        },
    ]

    with pytest.raises(Refusal, match="--approve-space"):
        Workflow._validate_native_inventory(rows, descriptor, set())

    evidence = Workflow._validate_native_inventory(rows, descriptor, {space})
    assert evidence["spaces"] == {space: [7]}
    rows[1]["workspace"] = "default"
    with pytest.raises(Refusal, match="unmanaged"):
        Workflow._validate_native_inventory(rows, descriptor, {space})


def test_new_receipt_is_exact_and_backend_bound():
    host = "11111111-1111-4111-8111-111111111111"
    space = "22222222-2222-4222-8222-222222222222"
    receipt = (
        f"dmux://{host}/spaces/{space}\tbackend=wez\tcreated=true\tconnected=false\treplayed=false"
    )
    assert Workflow._parse_new_receipt(receipt, backend="wez") == (host, space)
    with pytest.raises(Refusal):
        Workflow._parse_new_receipt(receipt, backend="tmux")
    with pytest.raises(Refusal):
        Workflow._parse_new_receipt(receipt + "\nnoise", backend="wez")


def test_remote_command_quotes_every_argument_and_rejects_host_injection():
    command = remote_argv("archie", ["printf", "%s", "a b;$(false)"])
    assert command[-1] == "printf %s 'a b;$(false)'"
    with pytest.raises(CommandError):
        remote_argv("archie;false", ["true"])


def test_runner_scrubs_ambient_mux_identity(monkeypatch):
    monkeypatch.setenv("TMUX", "forbidden")
    monkeypatch.setenv("WEZTERM_PANE", "99")

    result = Runner().capture(
        ["env"],
        env={"DMUX_WEZ_FIRST": "1"},
        unset_env=("TMUX", "WEZTERM_PANE"),
    )

    assert "TMUX=" not in result.stdout
    assert "WEZTERM_PANE=" not in result.stdout
    assert "DMUX_WEZ_FIRST=1" in result.stdout


class ReceiptRunner:
    def __init__(self, receipt):
        self.receipt = receipt
        self.calls = 0

    def capture(self, argv, **_kwargs):
        self.calls += 1
        return Result(tuple(argv), 0, self.receipt, "")


def test_smoke_creation_is_journaled_once_and_never_duplicated(tmp_path):
    item = release(tmp_path)
    host = "11111111-1111-4111-8111-111111111111"
    space = "22222222-2222-4222-8222-222222222222"
    runner = ReceiptRunner(
        f"dmux://{host}/spaces/{space}"
        "\tbackend=wez\tcreated=true\tconnected=false\treplayed=false\n"
    )
    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(
        store,
        runner,
        config(tmp_path, Path(item.data["frozen"]["dotfiles"]["repo"]), Path("/tmp/wezterm")),
    )
    with store.exclusive():
        store.create(item)
        workflow._ensure_primary_smoke(item)
        workflow._ensure_primary_smoke(item)

    assert runner.calls == 1
    assert item.data["smoke"]["space_uid"] == space
    assert item.completed("verify.smoke_identity")


def test_artifact_hash_drift_is_refused(tmp_path):
    path = tmp_path / "dmux"
    path.write_bytes(b"first")
    raw = {"dmux": {"path": str(path), "sha256": "0" * 64, "bytes": 5}}
    workflow = Workflow(
        RolloutStore(tmp_path / "state"),
        Runner(),
        config(tmp_path, Path("/tmp/dotfiles"), Path("/tmp/wezterm")),
    )

    with pytest.raises(Refusal, match="hash changed"):
        workflow._verify_artifact_set(raw, "test")


def test_dirty_detached_build_worktree_is_refused(tmp_path):
    repo = pushed_repo(tmp_path, "dotfiles")
    commit = git(repo, "rev-parse", "HEAD")
    item = release(tmp_path)
    item.data["frozen"]["dotfiles"].update({"repo": str(repo), "commit": commit})
    workflow = Workflow(
        RolloutStore(tmp_path / "state"), Runner(), config(tmp_path, repo, Path("/tmp/wezterm"))
    )
    (repo / "source.txt").write_text("dirty build\n", encoding="utf-8")

    with pytest.raises(Refusal, match="dirty or stale source build refused"):
        workflow._require_clean_frozen_worktree(item, "dotfiles", repo)


def test_mac_build_freezes_fork_before_running_dmux_gate(tmp_path, monkeypatch):
    item = release(tmp_path)
    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(
        store,
        Runner(),
        config(tmp_path, Path(item.data["frozen"]["dotfiles"]["repo"]), Path("/tmp/wezterm")),
    )
    calls = []

    monkeypatch.setattr(workflow, "_ensure_worktree", lambda _release, _source, path: path)
    monkeypatch.setattr(workflow, "_require_clean_frozen_worktree", lambda *_args: None)
    monkeypatch.setattr(
        workflow,
        "_build_mac_wezterm",
        lambda *_args, **_kwargs: calls.append("wezterm"),
    )
    monkeypatch.setattr(
        workflow,
        "_build_mac_dotfiles",
        lambda *_args, **_kwargs: calls.append("dotfiles"),
    )

    with store.exclusive():
        store.create(item)
        workflow.build(item)

    assert calls == ["wezterm", "dotfiles"]


def test_dmux_gate_uses_only_the_frozen_fork_binaries(tmp_path, monkeypatch):
    item = release(tmp_path)
    binary_dir = tmp_path / "artifacts" / "wezterm"
    binary_dir.mkdir(parents=True)
    wezterm = binary_dir / "wezterm"
    mux_server = binary_dir / "wezterm-mux-server"
    for binary in (wezterm, mux_server):
        binary.write_bytes(b"test")
        binary.chmod(0o755)
    item.data["artifacts"]["mac_wezterm"] = {
        "wezterm": {"path": str(wezterm)},
        "wezterm-mux-server": {"path": str(mux_server)},
    }
    monkeypatch.setenv("PATH", "/usr/bin")

    environment = Workflow._mac_dmux_test_environment(item, tmp_path / "cargo-target")

    assert environment["DMUX_TEST_FORK_WEZTERM"] == str(wezterm)
    assert environment["DMUX_TEST_FORK_MUX_SERVER"] == str(mux_server)
    assert environment["DMUX_TEST_REQUIRE_FORK"] == "1"
    assert environment["PATH"] == f"{binary_dir}:/usr/bin"


def test_archie_pacman_pause_is_exact_and_interactive(tmp_path):
    item = release(tmp_path)
    item.data["artifacts"]["archie_packages"] = {
        "main": {
            "path": "/home/fredrir/packages/release/wezterm-fredrir-git-1.470bd984-1-x86_64.pkg.tar.zst"
        },
        "debug": {
            "path": "/home/fredrir/packages/release/wezterm-fredrir-git-debug-1.470bd984-1-x86_64.pkg.tar.zst"
        },
    }
    workflow = Workflow(
        RolloutStore(tmp_path / "state"),
        Runner(),
        config(tmp_path, Path("/tmp/dotfiles"), Path("/tmp/wezterm")),
    )

    command = workflow.archie_install_command(item)

    assert command.startswith("ssh -t archie ")
    assert "sudo pacman -U" in command
    assert "--noconfirm" not in command
    assert command.count(".pkg.tar.zst") == 2


def test_archie_fork_gates_are_explicit_and_packaging_is_host_independent():
    dotfiles = Path("/release/dotfiles")
    wezterm = Path("/release/wezterm")
    target = Path("/release/targets/wezterm-gates")

    commands = Workflow._archie_wezterm_gate_commands(dotfiles, wezterm, target)

    assert len(commands) == 4
    assert [command[command.index("-p") + 1] for command in commands[:3]] == [
        "codec",
        "mux",
        "wezterm-gui",
    ]
    assert "dmux" in commands[2]
    assert commands[3][-2:] == [
        "sh",
        "/release/dotfiles/shared/wezterm/wez/dmux_bridge/tests/fork_surface.sh",
    ]
    for command in commands:
        assert f"CARGO_TARGET_DIR={target}" in command
        for name in AMBIENT_MUX_VARS:
            index = command.index(name)
            assert command[index - 1] == "-u"

    package = Workflow._archie_makepkg_command(Path("/release/packages"), Path("/release"))
    assert package[-1] == "--nocheck"
    assert package.count("--nocheck") == 1


def test_rollout_source_has_no_broad_process_kill():
    source = Path(Workflow.__module__.replace(".", "/") + ".py")
    text = (Path(__file__).parents[2] / "src" / source).read_text(encoding="utf-8")

    assert "pkill" not in text
    assert "killall" not in text
    assert "SIGKILL" not in text
    assert "os.kill(pid, signal.SIGTERM)" in text
