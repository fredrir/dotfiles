import hashlib
import json
import os
import shlex
import subprocess
from pathlib import Path

import pytest

from tools.dmux_rollout import model as rollout_model
from tools.dmux_rollout import service_env
from tools.dmux_rollout.command import Result, Runner, remote_argv
from tools.dmux_rollout.errors import CommandError, Refusal, StateError
from tools.dmux_rollout.storage import RolloutStore
from tools.dmux_rollout.workflow import (
    AMBIENT_MUX_VARS,
    ARCHIE_MUX_UNIT,
    MAC_ENV_LOADER_LABEL,
    Workflow,
    WorkflowConfig,
)

from .helpers import (
    DURABLE_WEZ_FIRST,
    NO_PREFERENCE,
    RUNTIME_ONLY,
    LaunchdFake,
    config,
    git,
    mac_workflow,
    pushed_repo,
    release,
    source,
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


def test_dmux_suite_seams_are_short_absolute_and_release_scoped(tmp_path):
    seams = Workflow._dmux_suite_seams(
        "20260823-7675b61-b9d8dfae-r6", tmp_path / "release", Path("/var/folders/xx/T")
    )
    assert set(seams) == {"DMUX_RUNTIME_DIR", "XDG_DATA_HOME", "XDG_STATE_HOME"}
    assert seams["DMUX_RUNTIME_DIR"] == "/var/folders/xx/T/dmux-r.d8dfaer6/rt"
    assert seams["XDG_DATA_HOME"] == str(tmp_path / "release/test-home/data")
    assert seams["XDG_STATE_HOME"] == str(tmp_path / "release/test-home/state")
    assert all(value.startswith("/") for value in seams.values())
    with pytest.raises(Refusal, match="too deep for a unix socket path"):
        Workflow._dmux_suite_seams("r", tmp_path, Path("/" + "x" * 95))


def test_runtime_dir_growth_names_only_new_entries(tmp_path):
    live = tmp_path / "live"
    (live / "bridge").mkdir(parents=True)
    (live / "wez-dmux.json").write_text("{}")
    before = Workflow._runtime_dir_entries(live)
    (live / "backend_1.lock").write_text("")
    (live / "bridge" / "key").write_text("k")
    (live / "wez-dmux.json").unlink()
    after = Workflow._runtime_dir_entries(live)
    assert Workflow._runtime_dir_growth(before, after) == ["backend_1.lock", "bridge/key"]
    assert Workflow._runtime_dir_entries(tmp_path / "absent") == []


class ArchieArchiveFake(Runner):
    """pacman -Q, test -f, sha256sum and stat over a fake Archie filesystem."""

    def __init__(self, files: dict[str, str]):
        self.files = files  # path -> sha256
        self.sent: list[list[str]] = []

    def capture(self, argv, **kwargs):
        argv = list(argv)
        self.sent.append(argv)
        remote = argv[argv.index("--") + 1] if "--" in argv else ""
        if "pacman -Q wezterm-fredrir-git-debug" in remote:
            return Result(tuple(argv), 0, "wezterm-fredrir-git-debug 1.0-1\n", "")
        if "pacman -Q wezterm-fredrir-git" in remote:
            return Result(tuple(argv), 0, "wezterm-fredrir-git 1.0-1\n", "")
        if remote.startswith("test -f "):
            path = remote.split(" ", 2)[2].strip("'")
            return Result(tuple(argv), 0 if path in self.files else 1, "", "")
        if remote.startswith("sha256sum"):
            path = remote.rsplit(" ", 1)[1].strip("'")
            return Result(tuple(argv), 0, f"{self.files[path]}  {path}\n", "")
        if remote.startswith("stat"):
            return Result(tuple(argv), 0, "7\n", "")
        return Result(tuple(argv), 0, "", "")


def _prior_release_with_archie_packages(store: RolloutStore, *, sha: str) -> dict[str, str]:
    prior = rollout_model.Release.create(
        release_id="20260817-prior",
        dotfiles=source(Path("/tmp/d"), "3" * 40),
        wezterm=source(Path("/tmp/w"), "4" * 40),
        smoke_name="rollout-smoke",
        archie_host="archie",
    )
    root = "/home/fredrir/packages/dmux-rollouts/20260817-prior/packages"
    paths = {
        "main": f"{root}/wezterm-fredrir-git-1.0-1-x86_64.pkg.tar.zst",
        "debug": f"{root}/wezterm-fredrir-git-debug-1.0-1-x86_64.pkg.tar.zst",
    }
    prior.data["artifacts"]["archie_packages"] = {
        key: {"path": path, "sha256": sha, "bytes": 7} for key, path in paths.items()
    }
    prior.checkpoint("deploy.archie.packages", {})
    store.create(prior)
    return paths


def test_archie_rollback_archives_come_from_the_release_that_installed_them(tmp_path):
    store = RolloutStore(tmp_path / "state")
    store.initialize()
    sha = "a" * 64
    paths = _prior_release_with_archie_packages(store, sha=sha)
    current = release(tmp_path)
    store.create(current)
    # The current release built packages with the very same filename.
    same_name = {
        path.replace("20260817-prior", current.release_id): "b" * 64 for path in paths.values()
    }
    runner = ArchieArchiveFake({**{p: sha for p in paths.values()}, **same_name})
    workflow = Workflow(
        store, runner, config(tmp_path, Path("/tmp/dotfiles"), Path("/tmp/wezterm"))
    )

    rows = workflow._archie_rollback_packages("archie", current=current)

    assert rows["wezterm-fredrir-git"]["path"] == paths["main"]
    assert rows["wezterm-fredrir-git-debug"]["path"] == paths["debug"]
    assert rows["wezterm-fredrir-git"]["sha256"] == sha
    assert rows["wezterm-fredrir-git"]["version"] == "1.0-1"

    # A replaced archive is refused, never silently reused.
    runner.files[paths["main"]] = "c" * 64
    with pytest.raises(Refusal, match="no longer matches the journal"):
        workflow._archie_rollback_packages("archie", current=current)


def test_archie_rollback_archives_fall_back_to_the_yay_cache_without_a_journal(tmp_path):
    store = RolloutStore(tmp_path / "state")
    store.initialize()
    current = release(tmp_path)
    store.create(current)
    cache = "/home/fredrir/.cache/yay/wezterm-fredrir-git"
    files = {
        f"{cache}/wezterm-fredrir-git-1.0-1-x86_64.pkg.tar.zst": "d" * 64,
        f"{cache}/wezterm-fredrir-git-debug-1.0-1-x86_64.pkg.tar.zst": "e" * 64,
    }
    workflow = Workflow(
        store,
        ArchieArchiveFake(files),
        config(tmp_path, Path("/tmp/dotfiles"), Path("/tmp/wezterm")),
    )

    rows = workflow._archie_rollback_packages("archie", current=current)
    assert rows["wezterm-fredrir-git"]["path"].startswith(cache)

    with pytest.raises(Refusal, match="absent: looked in"):
        Workflow(
            store,
            ArchieArchiveFake({}),
            config(tmp_path, Path("/tmp/dotfiles"), Path("/tmp/wezterm")),
        )._archie_rollback_packages("archie", current=current)


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
        "/release/dotfiles/shared/wezterm/wez/dmux_bridge/tests/suite.sh",
    ]
    assert f"DMUX_WEZTERM_SOURCE={wezterm}" in commands[3], (
        "the suite skips fork_surface.sh unless the frozen fork is named"
    )
    for command in commands:
        assert f"CARGO_TARGET_DIR={target}" in command
        for name in AMBIENT_MUX_VARS:
            index = command.index(name)
            assert command[index - 1] == "-u"

    package = Workflow._archie_makepkg_command(Path("/release/packages"), Path("/release"))
    assert package[-1] == "--nocheck"
    assert package.count("--nocheck") == 1


def test_both_wezterm_gates_run_the_dmux_suite():
    source = Path(Workflow.__module__.replace(".", "/") + ".py")
    text = (Path(__file__).parents[2] / "src" / source).read_text(encoding="utf-8")

    # The mac gate and the Archie gate must drive the same entry point. A suite
    # that runs on only one path is the gap this suite exists to close.
    assert text.count("dmux_bridge/tests/suite.sh") == 2
    assert "dmux_bridge/tests/fork_surface.sh" not in text, (
        "fork_surface.sh is reached through suite.sh; a direct call would drift from it"
    )


def test_rollout_source_has_no_broad_process_kill():
    source = Path(Workflow.__module__.replace(".", "/") + ".py")
    text = (Path(__file__).parents[2] / "src" / source).read_text(encoding="utf-8")

    assert "pkill" not in text
    assert "killall" not in text
    assert "SIGKILL" not in text
    assert "os.kill(pid, signal.SIGTERM)" in text


def dirt_witness(dirty, changed=()):
    return {"dirty": list(dirty), "changed": [{"status": "M", "path": p} for p in changed]}


def test_unrelated_dirt_appearing_beside_the_witness_is_reported_not_refused():
    config = dirt_witness([" M linux/common/gtk/gtk-3.0/colors.css"])
    dirty = [
        " M linux/common/gtk/gtk-3.0/colors.css",
        " M linux/common/gtk/gtk-3.0/settings.ini",
        "?? scratch/note.txt",
    ]

    appeared = Workflow._require_dirt_preserved(dirty, config, action="fast-forward")

    assert appeared == [" M linux/common/gtk/gtk-3.0/settings.ini", "?? scratch/note.txt"]


def test_losing_witnessed_dirt_is_refused():
    config = dirt_witness([" M shared/obsidian/hotkeys.json"])

    with pytest.raises(Refusal, match="did not preserve its pre-existing dirt"):
        Workflow._require_dirt_preserved([], config, action="fast-forward")


def test_new_dirt_on_a_release_managed_path_is_refused():
    config = dirt_witness([], changed=["shared/wezterm/wezterm.lua"])

    with pytest.raises(Refusal, match="left a release-managed path dirty"):
        Workflow._require_dirt_preserved(
            [" M shared/wezterm/wezterm.lua"], config, action="fast-forward"
        )


def test_unreadable_new_dirt_entry_is_refused():
    config = dirt_witness([])

    with pytest.raises(Refusal, match="unsupported Archie dirty entry"):
        Workflow._require_dirt_preserved(['?? "quoted path.txt"'], config, action="rollback")


def test_dirty_entry_path_rejects_renames_and_quoting():
    assert Workflow._dirty_entry_path(" M a/b.txt") == "a/b.txt"
    assert Workflow._dirty_entry_path("?? a/b.txt") == "a/b.txt"
    assert Workflow._dirty_entry_path('R  "a" -> "b"') is None
    assert Workflow._dirty_entry_path(" M ") is None
    assert Workflow._dirty_entry_path("M") is None


def test_archie_mux_unit_matches_the_unit_file_the_repo_installs():
    root = Path(__file__).parents[3].parent
    units = sorted((root / "linux/arch/wezterm-mux").glob("*.service"))

    assert [unit.name for unit in units] == [ARCHIE_MUX_UNIT]


def _quit_workflow(tmp_path):
    """A workflow whose only child command is the quit trigger, recorded."""
    sent = []

    class Recorder(Runner):
        def capture(self, argv, **kwargs):
            sent.append(argv)
            return Result(argv=argv, returncode=0, stdout="", stderr="")

    workflow = Workflow(
        RolloutStore(tmp_path / "state"),
        Recorder(),
        config(tmp_path, tmp_path / "dotfiles", tmp_path / "wezterm"),
    )
    gui = {"pid": 4242, "heartbeat": str(tmp_path / "missing.json"), "gui_instance": "gui-x"}
    return workflow, gui, sent


def test_managed_quit_confirms_frontmost_before_sending_cmd_q(tmp_path):
    workflow, gui, sent = _quit_workflow(tmp_path)

    with pytest.raises(Refusal):
        workflow._safe_quit_gui(gui, mechanism="keystroke", timeout=0)

    assert sent[0][:2] == ["osascript", "-e"]
    script = sent[0][-1]
    assert script.index("frontmost of target") < script.index('keystroke "q"')
    assert "never became frontmost" in script
    assert "unix id is 4242" in script


def test_default_managed_quit_addresses_one_pid_and_never_launchservices(tmp_path):
    workflow, gui, sent = _quit_workflow(tmp_path)

    with pytest.raises(Refusal):
        workflow._safe_quit_gui(gui, timeout=0)

    assert sent[0][:3] == ["osascript", "-l", "JavaScript"]
    script = sent[0][-1]
    # The managed GUI is exec'd from inside the bundle rather than launched
    # through LaunchServices, so it registers no bundle identifier. A
    # `tell application` form would miss it and launch a second instance.
    assert "tell application" not in script
    assert "runningApplicationWithProcessIdentifier" in script
    assert "var pid = 4242;" in script
    # JXA bridges a zero-argument ObjC method as a property, so reading
    # `app.terminate` is what sends the event. Adding the parentheses that
    # every reader expects sends it and then fails calling the returned
    # boolean, which exits nonzero on every run. Pin the no-paren form.
    assert "var sent = app.terminate;" in script
    assert "app.terminate()" not in script
    assert script.index("app.isNil()") < script.index("app.terminate")
    # A refused send is silent otherwise, so it has to be turned into an error.
    assert "if (!sent)" in script
    # No frontmost, no Space switch, no keystroke: that is the whole point.
    assert "frontmost" not in script
    assert "keystroke" not in script


def test_managed_quit_names_the_mechanism_that_failed(tmp_path):
    workflow, gui, _ = _quit_workflow(tmp_path)

    with pytest.raises(Refusal, match="managed application_quit did not reach"):
        workflow._safe_quit_gui(gui, timeout=0)
    with pytest.raises(Refusal, match="managed keystroke did not reach"):
        workflow._safe_quit_gui(gui, mechanism="keystroke", timeout=0)
    with pytest.raises(Refusal, match="unknown managed safe quit mechanism"):
        workflow._safe_quit_gui(gui, mechanism="sigterm")


def _keybinding_workflow(tmp_path, *, keystroke_fails):
    """A workflow with the keybinding gate's collaborators stubbed out.

    Only the gate's own control flow is under test here; presentation, owner
    snapshots and the heartbeat postcondition each have their own coverage.
    """
    calls = []
    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(
        store,
        Runner(),
        config(tmp_path, tmp_path / "dotfiles", tmp_path / "wezterm"),
    )
    owner = {
        "pid": 1,
        "epoch": "epoch-uid",
        "backend_instance_uid": "backend-uid",
        "socket_dev": 42,
        "socket_ino": 84,
        "spaces": {"space-uid": [7]},
    }
    gui = {"pid": 4242, "gui_instance": "gui-x"}

    def safe_quit(target, *, mechanism="application_quit", timeout=30.0):
        calls.append(mechanism)
        if mechanism == "keystroke" and keystroke_fails:
            raise Refusal("GUI 4242 never became frontmost")
        return {"gui_instance": "gui-x", "mechanism": mechanism}

    workflow._mac_owner_snapshot = lambda **kwargs: owner
    workflow._present_and_wait = lambda *a, **kw: gui
    workflow._live_gui_for_space = lambda *a, **kw: gui
    workflow._safe_quit_gui = safe_quit
    return workflow, store, calls


PRIMARY = {"name": "rollout-smoke", "host_uid": "host-uid", "space_uid": "space-uid"}


def _run_keybinding_gate(workflow, store, tmp_path):
    item = release(tmp_path)
    with store.exclusive():
        store.create(item)
        workflow._verify_lifecycle_keybinding(item, {"space-uid"}, PRIMARY)
    return item.checkpoints["verify.lifecycle.keybinding"]["evidence"]


def test_keybinding_gate_records_a_skip_instead_of_failing_the_release(tmp_path):
    workflow, store, calls = _keybinding_workflow(tmp_path, keystroke_fails=True)

    evidence = _run_keybinding_gate(workflow, store, tmp_path)
    assert "never became frontmost" in evidence["skipped"]
    assert evidence["mechanism"] == "keystroke"
    # A failed keystroke leaves the presentation's domain attached, which the
    # next deployment refuses. The gate must put it back.
    assert calls == ["keystroke", "application_quit"]


def test_keybinding_gate_does_not_re_quit_when_the_keystroke_worked(tmp_path):
    workflow, store, calls = _keybinding_workflow(tmp_path, keystroke_fails=False)

    evidence = _run_keybinding_gate(workflow, store, tmp_path)

    assert evidence["skipped"] is None
    assert calls == ["keystroke"]


def test_managed_quit_reads_the_heartbeat_before_giving_up(tmp_path):
    """A zero timeout is one attempt, not none.

    The postcondition is the only proof the quit worked, so the poll loop may
    never exit without having looked at the heartbeat at least once.
    """
    workflow, gui, _ = _quit_workflow(tmp_path)
    reads = []

    def record(path, *, maximum):
        reads.append(Path(path))
        raise Refusal("heartbeat is absent")

    workflow._load_bounded_json = record

    with pytest.raises(Refusal):
        workflow._safe_quit_gui(gui, timeout=0)

    assert reads == [Path(gui["heartbeat"])]


def test_plan_freezes_an_explicit_archie_route_and_refuses_changing_it(tmp_path):
    dotfiles = pushed_repo(tmp_path, "dotfiles")
    wezterm = pushed_repo(tmp_path, "wezterm")
    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(store, Runner(), config(tmp_path, dotfiles, wezterm))

    with store.exclusive():
        first = workflow.plan(release_id="r6", smoke_name="smoke", archie_ssh="fredrir@10.77.77.2")
    assert first.data["hosts"]["archie"]["ssh"] == "fredrir@10.77.77.2"

    # Re-planning without the flag reuses the frozen route; naming another
    # route for the same release is a different release.
    with store.exclusive():
        again = workflow.plan(release_id="r6", smoke_name="smoke")
    assert again.data["hosts"]["archie"]["ssh"] == "fredrir@10.77.77.2"
    with store.exclusive(), pytest.raises(Refusal, match="already addresses Archie as"):
        workflow.plan(release_id="r6", smoke_name="smoke", archie_ssh="archie")

    # The route is one ssh destination token, validated like every remote argv.
    with store.exclusive(), pytest.raises(CommandError, match="SSH host"):
        workflow.plan(release_id="r7", smoke_name="smoke", archie_ssh="fredrir@10.77.77.2;id")
    with store.exclusive(), pytest.raises(CommandError, match="SSH host"):
        workflow.plan(release_id="r7", smoke_name="smoke", archie_ssh="-oProxyCommand=id")

    # Old manifests and flag-less plans keep the historical default.
    with store.exclusive():
        default = workflow.plan(release_id="r7", smoke_name="smoke")
    assert default.data["hosts"]["archie"]["ssh"] == "archie"
    assert default.archie_dmux_host == "archie"
    del default.data["hosts"]["archie"]["dmux_host"]  # an r5-shaped manifest
    assert default.archie_dmux_host == "archie"

    # The ssh route is never what `dmux --host` is given: that is a separate,
    # separately validated selector (alias/label/HostUid/legacy name).
    with store.exclusive():
        named = workflow.plan(
            release_id="r8",
            smoke_name="smoke",
            archie_ssh="fredrir@10.77.77.2",
            archie_dmux_host="b",
        )
    assert named.archie_dmux_host == "b"
    with store.exclusive(), pytest.raises(Refusal, match="already names Archie's dmux host"):
        workflow.plan(release_id="r8", smoke_name="smoke", archie_dmux_host="archie")
    with store.exclusive(), pytest.raises(StateError, match="enrolled alias/label"):
        workflow.plan(release_id="r9", smoke_name="smoke", archie_dmux_host="fredrir@10.77.77.2")


def test_archie_steps_address_the_route_frozen_in_the_manifest(tmp_path):
    item = release(tmp_path)
    item.data["hosts"]["archie"] = {"ssh": "fredrir@10.77.77.2", "dmux_host": "b"}
    item.data["artifacts"]["archie_packages"] = {
        "main": {"path": "/pkgs/wezterm-fredrir-git-1.11111111-1-x86_64.pkg.tar.zst"},
        "debug": {"path": "/pkgs/wezterm-fredrir-git-debug-1.11111111-1-x86_64.pkg.tar.zst"},
    }
    host_uid = "11111111-1111-4111-8111-111111111111"
    space_uid = "22222222-2222-4222-8222-222222222222"
    sent = []

    class Recorder(Runner):
        def capture(self, argv, **kwargs):
            sent.append(list(argv))
            if "new" in argv:
                receipt = (
                    f"dmux://{host_uid}/spaces/{space_uid}\tbackend=wez\tcreated=true"
                    "\tconnected=false\treplayed=false\n"
                )
                return Result(tuple(argv), 0, receipt, "")
            if "pacman" in argv[-1]:
                return Result(tuple(argv), 0, "wezterm-fredrir-git 1.11111111-1\n", "")
            return Result(tuple(argv), 0, "", "")

    store = RolloutStore(tmp_path / "state")
    workflow = Workflow(store, Recorder(), config(tmp_path, tmp_path / "d", tmp_path / "w"))
    snapshots = []

    def archie_owner(release, **kwargs):
        snapshots.append(release.data["hosts"]["archie"]["ssh"])
        return {"spaces": {space_uid: [3]}}

    workflow._archie_owner_snapshot = archie_owner

    assert workflow.archie_install_command(item).startswith("ssh -t fredrir@10.77.77.2 ")
    assert workflow._archie_packages_installed(item) is False
    assert sent[-1][:4] == ["ssh", "-o", "BatchMode=yes", "fredrir@10.77.77.2"]

    with store.exclusive():
        store.create(item)
        item.checkpoints["verify.two_host"] = {"at": "2026-08-23T00:00:00Z", "evidence": {}}
        workflow._verify_two_host(item)

    new_call = next(argv for argv in sent if "new" in argv)
    assert new_call[new_call.index("--host") + 1] == "b", "dmux --host takes the selector"
    assert "fredrir@10.77.77.2" not in new_call
    assert item.data["smoke"]["remote"]["space_uid"] == space_uid
    assert snapshots == ["fredrir@10.77.77.2"]


# Durable enablement (ADR 012 WS-F.1) ----------------------------------------


REPO_ROOT = Path(__file__).parents[3].parent


def test_service_env_grammar_matches_the_shell_helper_the_repo_installs(tmp_path):
    helper = REPO_ROOT / "shared/wezterm/mux/dmux-service-env.sh"
    text = helper.read_text(encoding="utf-8")

    # The character sets are the grammar; compare them literally.
    assert f"DMUX_SERVICE_ENV_KEY_CHARS='{service_env.KEY_CHARS}'" in text
    assert f"DMUX_SERVICE_ENV_VALUE_CHARS='{service_env.VALUE_CHARS}'" in text
    assert "/.config/dmux/service.env" in text
    assert service_env.MAC_RELATIVE_PATH == ".config/dmux/service.env"
    # The file paths and the loader's label come from the tracked files.
    assert (
        WorkflowConfig(
            dotfiles_repo=Path("/d"), wezterm_repo=Path("/w"), packages_root=Path("/p")
        ).mac_service_env
        == Path.home() / service_env.MAC_RELATIVE_PATH
    )
    plist = (REPO_ROOT / f"macos/launchd/{MAC_ENV_LOADER_LABEL}.plist").read_text()
    assert f"<string>{MAC_ENV_LOADER_LABEL}</string>" in plist
    assert "dmux-env-load.sh" in plist
    unit = (REPO_ROOT / "linux/arch/wezterm-mux" / ARCHIE_MUX_UNIT).read_text()
    assert f"~/{service_env.LINUX_RELATIVE_PATH}" in unit

    # Behavioural parity: what the tool renders, the helper reads back the
    # same way, and what the helper refuses, the tool refuses.
    rendered = service_env.render(
        "# note\n  DMUX_LEGACY_POLICY=0\nDMUX_WEZ_FIRST=0\n",
        {"DMUX_WEZ_FIRST": "1"},
        name="x",
    )
    assert rendered == "# note\n  DMUX_LEGACY_POLICY=0\nDMUX_WEZ_FIRST=1\n"
    good = tmp_path / "service.env"
    good.write_text(rendered, encoding="utf-8")
    bad = tmp_path / "bad.env"
    bad.write_text("DMUX_WEZ_FIRST=1\nDMUX_X=$(id)\n", encoding="utf-8")
    script = (
        f'. "{helper}" && lines=$(dmux_service_env_lines "$1") && '
        'dmux_service_env_lookup DMUX_WEZ_FIRST "$lines" && '
        'dmux_service_env_lookup DMUX_LEGACY_POLICY "$lines"'
    )
    shell = subprocess.run(
        ["/bin/sh", "-c", script, "sh", str(good)], capture_output=True, text=True, check=False
    )
    assert shell.returncode == 0, shell.stderr
    assert shell.stdout == "1\n0\n"
    assert service_env.parse(rendered, name="x") == {
        "DMUX_LEGACY_POLICY": "0",
        "DMUX_WEZ_FIRST": "1",
    }
    refused = subprocess.run(
        ["/bin/sh", "-c", script, "sh", str(bad)], capture_output=True, text=True, check=False
    )
    assert refused.returncode != 0
    assert "bad.env:2:" in refused.stderr
    with pytest.raises(Refusal, match="line 2: value must match"):
        service_env.parse(bad.read_text(), name="bad.env")


def test_service_env_refuses_whole_files_and_keeps_the_last_assignment():
    with pytest.raises(Refusal, match=r"line 1: key must start with DMUX_"):
        service_env.parse("PATH=/bin\nDMUX_WEZ_FIRST=1\n", name="f")
    with pytest.raises(Refusal, match=r"line 2: value must match"):
        service_env.parse("DMUX_WEZ_FIRST=1\nDMUX_WEZ_FIRST=1\r\n", name="f")
    with pytest.raises(Refusal, match="expected KEY=VALUE"):
        service_env.parse("DMUX_WEZ_FIRST\n", name="f")
    assert service_env.parse("DMUX_WEZ_FIRST=0\nDMUX_WEZ_FIRST=1\n", name="f") == {
        "DMUX_WEZ_FIRST": "1"
    }
    # A file the tool cannot vouch for is never rewritten.
    with pytest.raises(Refusal, match="malformed"):
        service_env.render("DMUX_X=a b\n", {"DMUX_WEZ_FIRST": "1"}, name="f")
    with pytest.raises(Refusal, match="does not match the file grammar"):
        service_env.render("", {"DMUX_WEZ_FIRST": "1; id"}, name="f")
    assert (
        service_env.without(
            "DMUX_LEGACY_POLICY=1\n# k\nDMUX_WEZ_FIRST=1\n", {"DMUX_LEGACY_POLICY"}, name="f"
        )
        == "# k\nDMUX_WEZ_FIRST=1\n"
    )
    assert service_env.without("DMUX_LEGACY_POLICY=1\n", {"DMUX_LEGACY_POLICY"}, name="f") == ""


def _env_workflow(tmp_path):
    cfg = config(tmp_path, tmp_path / "dotfiles", tmp_path / "wezterm")
    fake = LaunchdFake(cfg.mac_service_env)
    return Workflow(RolloutStore(tmp_path / "state"), fake, cfg), fake


def test_deploy_mac_refuses_without_the_env_loader_agent_and_names_the_operator_steps(tmp_path):
    workflow, fake = _env_workflow(tmp_path)
    item = release(tmp_path)
    workflow._require_live_config_matches = lambda _release: None

    with pytest.raises(Refusal) as refusal:
        workflow.deploy_mac(item)

    text = str(refusal.value)
    assert "never links dotfiles" in text
    assert "`dotfile link`" in text
    assert f"launchctl bootstrap gui/{os.getuid()} {workflow.config.mac_env_loader_plist}" in text
    # Nothing was installed or backed up before the refusal.
    assert item.checkpoints == {}
    assert fake.sent == []


def test_mac_durable_enablement_writes_the_file_then_proves_launchd_before_the_mux(tmp_path):
    workflow, fake = _env_workflow(tmp_path)
    fake.path.parent.mkdir(parents=True)
    fake.path.write_text("# mine\nDMUX_LEGACY_POLICY=0\n", encoding="utf-8")

    written = workflow._enable_mac_durable_flag()

    assert fake.path.read_text() == "# mine\nDMUX_LEGACY_POLICY=0\nDMUX_WEZ_FIRST=1\n"
    assert (fake.path.stat().st_mode & 0o777) == 0o600
    assert written["assignments"] == {"DMUX_LEGACY_POLICY": "0", "DMUX_WEZ_FIRST": "1"}
    assert fake.session["DMUX_WEZ_FIRST"] == "1"
    kinds = [argv[1] for argv in fake.sent if argv[0] == "launchctl"]
    assert kinds[0] == "kickstart"
    assert fake.sent[0][-1] == f"gui/{os.getuid()}/{MAC_ENV_LOADER_LABEL}"
    assert "getenv" in kinds
    assert "setenv" not in kinds

    # A line WS-F.2's by-hand repair already wrote is kept byte for byte.
    before = fake.path.read_bytes()
    again = workflow._enable_mac_durable_flag()
    assert again["unchanged"] is True
    assert fake.path.read_bytes() == before

    # The loader applied nothing: the tool says so instead of restarting the mux.
    fake.session.clear()
    fake.path.write_text("DMUX_WEZ_FIRST=1\n", encoding="utf-8")
    original = fake.capture

    def loader_fails(argv, **kwargs):
        if list(argv)[:2] == ["launchctl", "kickstart"]:
            return Result(tuple(argv), 0, "", "")
        return original(argv, **kwargs)

    fake.capture = loader_fails
    with pytest.raises(Refusal, match="launchd carries DMUX_WEZ_FIRST='', expected '1'"):
        workflow._require_launchd_env("DMUX_WEZ_FIRST", "1", timeout=0)


def test_mac_rollback_restores_the_env_file_and_unsets_what_it_no_longer_states(tmp_path):
    workflow, fake = _env_workflow(tmp_path)

    # Absent before deployment: rollback removes the file the tool created.
    absent = workflow._env_file_backup(fake.path)
    assert absent == {"path": str(fake.path), "absent": True, "content": None, "sha256": None}
    workflow._enable_mac_durable_flag()
    assert fake.session == {"DMUX_WEZ_FIRST": "1"}
    restored = workflow._restore_mac_env_file(absent)
    assert not fake.path.exists()
    assert fake.session == {}
    assert restored["assignments"] == {}
    assert ["launchctl", "unsetenv", "DMUX_WEZ_FIRST"] in fake.sent

    # Present before deployment: its exact bytes come back, and the session
    # carries exactly what it states.
    fake.path.parent.mkdir(parents=True, exist_ok=True)
    fake.path.write_text("DMUX_WEZ_FIRST=0\n", encoding="utf-8")
    present = workflow._env_file_backup(fake.path)
    workflow._enable_mac_durable_flag()
    assert fake.path.read_text() == "DMUX_WEZ_FIRST=1\n"
    workflow._restore_mac_env_file(present)
    assert fake.path.read_text() == "DMUX_WEZ_FIRST=0\n"
    assert fake.session == {"DMUX_WEZ_FIRST": "0"}

    # A manifest from before durable enablement cannot be rolled back blind.
    item = release(tmp_path)
    item.data["rollback"]["mac"] = {"files": [], "launchd_dmux_wez_first": ""}
    with pytest.raises(StateError, match="predates durable enablement"):
        workflow._require_mac_env_backup(item)


class ArchieFake(Runner):
    """ssh/scp against a remote file tree with a systemd environment block."""

    def __init__(self, conf):
        self.conf = str(conf)
        self.files = {}
        self.environment = {}
        self.sent = []

    def capture(self, argv, **kwargs):
        argv = list(argv)
        self.sent.append(argv)
        if argv[0] == "scp":
            self.files[argv[-1].partition(":")[2]] = Path(argv[-2]).read_text()
            return Result(tuple(argv), 0, "", "")
        assert argv[:3] == ["ssh", "-o", "BatchMode=yes"], argv
        remote = shlex.split(argv[-1])
        if remote[:2] == ["test", "-f"] or remote[:2] == ["test", "-e"]:
            return Result(tuple(argv), 0 if remote[2] in self.files else 1, "", "")
        if remote[0] == "cat":
            return Result(tuple(argv), 0, self.files[remote[-1]], "")
        if remote[0] == "sha256sum":
            digest = hashlib.sha256(self.files[remote[-1]].encode()).hexdigest()
            return Result(tuple(argv), 0, f"{digest}  {remote[-1]}\n", "")
        if remote[:2] == ["stat", "-c"] and remote[2] == "%s":
            return Result(tuple(argv), 0, f"{len(self.files[remote[-1]])}\n", "")
        if remote[:2] == ["stat", "-c"]:
            return Result(tuple(argv), 0, "fredrir:directory\n", "")
        if remote[0] == "mv":
            self.files[remote[-1]] = self.files.pop(remote[-2])
        if remote[:2] == ["rm", "-f"]:
            self.files.pop(remote[-1], None)
        if remote[:3] == ["systemctl", "--user", "daemon-reload"] and self.conf in self.files:
            self.environment.update(service_env.parse(self.files[self.conf], name="fake"))
        if remote[:3] == ["systemctl", "--user", "unset-environment"]:
            self.environment.pop(remote[3], None)
        if remote[:3] == ["systemctl", "--user", "show-environment"]:
            lines = "".join(f"{k}={v}\n" for k, v in self.environment.items())
            return Result(tuple(argv), 0, lines, "")
        return Result(tuple(argv), 0, "", "")


def test_archie_durable_enablement_writes_environment_d_and_proves_systemd(tmp_path):
    item = release(tmp_path)
    item.data["hosts"]["archie"]["ssh"] = "fredrir@10.77.77.2"
    base = config(tmp_path, tmp_path / "dotfiles", tmp_path / "wezterm")
    cfg = WorkflowConfig(**{**base.__dict__, "archie_home": tmp_path / "archie-home"})
    fake = ArchieFake(cfg.archie_env_file)
    workflow = Workflow(RolloutStore(tmp_path / "state"), fake, cfg)
    assert cfg.archie_env_file == tmp_path / "archie-home/.config/environment.d/50-dmux.conf"

    absent = workflow._remote_env_file_backup("fredrir@10.77.77.2", cfg.archie_env_file)
    assert absent["absent"] is True

    written = workflow._set_archie_env(item, {"DMUX_WEZ_FIRST": "1"})

    assert fake.files[str(cfg.archie_env_file)] == "DMUX_WEZ_FIRST=1\n"
    assert written["assignments"] == {"DMUX_WEZ_FIRST": "1"}
    assert fake.environment == {"DMUX_WEZ_FIRST": "1"}
    remote = [shlex.split(argv[-1]) for argv in fake.sent if argv[0] == "ssh"]
    verbs = [" ".join(r[:3]) for r in remote]
    assert "systemctl --user set-environment" not in verbs
    chmod = next(i for i, r in enumerate(remote) if r[0] == "chmod")
    move = next(i for i, r in enumerate(remote) if r[0] == "mv")
    reload = next(
        i for i, r in enumerate(remote) if r[:3] == ["systemctl", "--user", "daemon-reload"]
    )
    shown = next(i for i, r in enumerate(remote) if r[2:3] == ["show-environment"])
    assert chmod < move < reload < shown
    assert remote[chmod][1] == "0600"
    assert all(argv[3] == "fredrir@10.77.77.2" for argv in fake.sent if argv[0] == "ssh")

    # Rollback to "absent" removes the file and the manager's copy of the flag.
    restored = workflow._restore_archie_env_file(item, absent)
    assert str(cfg.archie_env_file) not in fake.files
    assert fake.environment == {}
    assert restored["assignments"] == {}
    assert ["systemctl", "--user", "unset-environment", "DMUX_WEZ_FIRST"] in remote + [
        shlex.split(argv[-1]) for argv in fake.sent[len(remote) :] if argv[0] == "ssh"
    ]


def test_rollout_source_never_sets_the_flag_runtime_only():
    source = Path(Workflow.__module__.replace(".", "/") + ".py")
    text = (Path(__file__).parents[2] / "src" / source).read_text(encoding="utf-8")

    # launchctl setenv / systemctl set-environment do not survive a reboot;
    # the file does. The tool may only ever unset a runtime copy (rollback).
    assert '"setenv"' not in text
    assert '"set-environment"' not in text
    assert '"import-environment"' not in text


# dmux doctor beside every owner snapshot -----------------------------------


def test_owner_snapshot_carries_doctor_and_checkpoints_store_it_beside_the_release(tmp_path):
    workflow, fake, store = mac_workflow(tmp_path)
    item = release(tmp_path)

    snapshot = workflow._mac_owner_snapshot(approved_spaces=set(), require_quiet=True)

    assert snapshot["host"] == "mac"
    assert snapshot["doctor"] == fake.doctor_document()
    assert fake.doctor_calls == 1
    doctor_argv = next(argv for argv in fake.sent if argv[1:2] == ["doctor"])
    assert doctor_argv[1:] == ["doctor", "--format", "json"]

    with store.exclusive():
        store.create(item)
        assert workflow._checkpoint(item, "deploy.mac.preflight", snapshot) is True
        after = workflow._mac_owner_snapshot(approved_spaces=set(), require_quiet=True)
        workflow._checkpoint(item, "verify.recovery", {"before": snapshot, "after": after})
        # A snapshot without a doctor document (a stubbed one, an old
        # manifest) stores nothing and breaks nothing.
        workflow._checkpoint(item, "plain", {"owner": {"pid": 1}})

    root = workflow._artifact_root(item) / "doctor"
    assert sorted(path.name for path in root.iterdir()) == [
        "deploy.mac.preflight-mac.json",
        "verify.recovery.after-mac.json",
        "verify.recovery.before-mac.json",
    ]
    stored = root / "deploy.mac.preflight-mac.json"
    assert (stored.stat().st_mode & 0o777) == 0o600
    assert json.loads(stored.read_text()) == fake.doctor_document()
    evidence = item.checkpoints["deploy.mac.preflight"]["evidence"]
    assert evidence["doctor"]["artifact"] == str(stored)
    assert evidence["doctor"]["wez_first"] == DURABLE_WEZ_FIRST
    assert "result" not in evidence["doctor"], "the manifest references the document, not a copy"
    # The caller's snapshot is left whole, so a later checkpoint of the same
    # snapshot stores its own copy of the document.
    assert snapshot["doctor"] == fake.doctor_document()
    listing = item.data["artifacts"]["doctor"]
    assert listing["deploy.mac.preflight-mac"] == {
        "path": str(stored),
        "sha256": evidence["doctor"]["sha256"],
        "checkpoint": "deploy.mac.preflight",
        "host": "mac",
    }
    assert set(listing) == {
        "deploy.mac.preflight-mac",
        "verify.recovery.before-mac",
        "verify.recovery.after-mac",
    }
    # The manifest with the listing still validates and reloads.
    assert store.load(item.release_id).data["artifacts"]["doctor"] == listing


def test_wait_for_the_owner_takes_doctor_once_after_the_postcondition_holds(tmp_path):
    workflow, fake, _ = mac_workflow(tmp_path)

    row = workflow._wait_mac_owner(
        lambda row: True, approved_spaces=set(), require_quiet=True, timeout=5
    )

    assert row["doctor"] == fake.doctor_document()
    assert fake.doctor_calls == 1


def test_doctor_verdicts_are_read_from_the_probe_and_states_are_opaque():
    def document(ok, detail, **extra):
        return {"action": "doctor", "result": {"wez_first": {"ok": ok, "detail": detail}, **extra}}

    assert Workflow._doctor_wez_first(document(True, DURABLE_WEZ_FIRST)) == (
        True,
        DURABLE_WEZ_FIRST,
    )
    assert Workflow._doctor_wez_first(document(True, NO_PREFERENCE))[0] is False
    assert Workflow._doctor_wez_first(document(False, RUNTIME_ONLY))[0] is False
    legacy = DURABLE_WEZ_FIRST.replace("Wez-first", "legacy")
    assert Workflow._doctor_wez_first(document(True, legacy))[0] is False
    with pytest.raises(Refusal, match="no wez_first probe"):
        Workflow._doctor_wez_first({"action": "doctor", "result": {}})

    # backend_instances is the B agent's field; absent today, opaque later.
    assert Workflow._doctor_states(document(True, DURABLE_WEZ_FIRST)) is None
    rows = [{"backend": "wez", "state": "E"}, {"backend": "tmux", "state": "F"}]
    assert Workflow._doctor_states(document(True, DURABLE_WEZ_FIRST, backend_instances=rows)) == [
        "E",
        "F",
    ]
    with pytest.raises(Refusal, match="did not return a doctor document"):
        Workflow._require_doctor_document({"action": "ls", "result": {}}, "Mac")


# Canary (plan §21 step 7 as amended) -----------------------------------------


def _deployed_mac_release(tmp_path, store, workflow):
    item = release(tmp_path)
    with store.exclusive():
        store.create(item)
        item.set_phase("verified")
        workflow._checkpoint(item, "deploy.mac.service", {"pid": 1, "epoch": "e"})
        store.save(item)
    return item


def _clock(monkeypatch, start="2026-08-23T10:00:00Z"):
    now = {"at": start}
    monkeypatch.setattr(rollout_model, "utc_now", lambda: now["at"])
    return now


def test_canary_start_needs_a_durable_flag_and_records_the_wall_clock_floor(tmp_path, monkeypatch):
    workflow, fake, store = mac_workflow(tmp_path)
    item = _deployed_mac_release(tmp_path, store, workflow)
    _clock(monkeypatch)

    fake.doctor_ok, fake.doctor_detail = False, RUNTIME_ONLY
    with store.exclusive(), pytest.raises(Refusal, match="not durably Wez-first.*runtime-only"):
        workflow.canary_start(item, "mac")
    assert "canary.mac.start" not in item.checkpoints
    fake.doctor_ok, fake.doctor_detail = True, NO_PREFERENCE
    with store.exclusive(), pytest.raises(Refusal, match="no preference stated"):
        workflow.canary_start(item, "mac")

    fake.doctor_detail = DURABLE_WEZ_FIRST
    with store.exclusive():
        workflow.canary_start(item, "mac")
        workflow.canary_start(item, "mac")  # idempotent

    row = item.checkpoints["canary.mac.start"]["evidence"]
    assert row["started_at"] == "2026-08-23T10:00:00Z"
    assert row["floor_at"] == "2026-08-24T10:00:00Z"
    assert row["owner"]["pid"] == fake.pid
    assert row["owner"]["doctor"]["artifact"].endswith("doctor/canary.mac.start.owner-mac.json")
    assert item.phase == "canary_mac"
    with store.exclusive(), pytest.raises(Refusal, match="needs completed checkpoint"):
        workflow.canary_start(item, "archie")
    with pytest.raises(StateError, match="unknown host"):
        workflow.canary_start(item, "macie")


def test_canary_end_refuses_before_the_floor_and_names_the_remaining_time(tmp_path, monkeypatch):
    workflow, fake, store = mac_workflow(tmp_path)
    item = _deployed_mac_release(tmp_path, store, workflow)
    now = _clock(monkeypatch)
    with store.exclusive():
        workflow.canary_start(item, "mac")

    now["at"] = "2026-08-24T09:30:00Z"
    with (
        store.exclusive(),
        pytest.raises(Refusal, match=r"floor ends at 2026-08-24T10:00:00Z \(0:30:00 remaining"),
    ):
        workflow.canary_end(item, "mac")
    assert "canary.mac.end" not in item.checkpoints

    now["at"] = "2026-08-24T12:00:00Z"
    with store.exclusive():
        workflow.canary_end(item, "mac")
    row = item.checkpoints["canary.mac.end"]["evidence"]
    assert row["ended_at"] == "2026-08-24T12:00:00Z"
    assert row["elapsed_hours"] == 26.0
    assert row["reboots"] == []
    assert row["start_owner"]["pid"] == row["owner"]["pid"] == fake.pid
    assert row["owner"]["doctor"]["artifact"].endswith("canary.mac.end.owner-mac.json")
    assert row["start_owner"]["doctor"]["artifact"].endswith("canary.mac.start.owner-mac.json")


def test_canary_end_refuses_an_unrecorded_restart_and_accepts_a_journaled_reboot(
    tmp_path, monkeypatch
):
    workflow, fake, store = mac_workflow(tmp_path)
    item = _deployed_mac_release(tmp_path, store, workflow)
    now = _clock(monkeypatch)
    with store.exclusive():
        workflow.canary_start(item, "mac")
        with pytest.raises(Refusal, match="has not restarted since canary.mac.start"):
            workflow.canary_reboot_observed(item, "mac")

    fake.restart()
    now["at"] = "2026-08-24T12:00:00Z"
    with store.exclusive(), pytest.raises(Refusal, match="restarted without a recorded reboot"):
        workflow.canary_end(item, "mac")

    with store.exclusive():
        assert workflow.canary_reboot_observed(item, "mac") == "canary.mac.reboot.1"
    reboot = item.checkpoints["canary.mac.reboot.1"]["evidence"]
    assert reboot["enablement_survived"] is True
    assert reboot["before_incarnation"]["pid"] == fake.pid - 1
    assert reboot["owner"]["pid"] == fake.pid
    assert reboot["owner"]["doctor"]["artifact"].endswith("canary.mac.reboot.1.owner-mac.json")

    # A second restart after the journaled one is again unexplained.
    fake.restart()
    with store.exclusive(), pytest.raises(Refusal, match="canary.mac.reboot.1 saw pid"):
        workflow.canary_end(item, "mac")
    with store.exclusive():
        assert workflow.canary_reboot_observed(item, "mac") == "canary.mac.reboot.2"
        workflow.canary_end(item, "mac")
    assert item.checkpoints["canary.mac.end"]["evidence"]["reboots"] == [
        "canary.mac.reboot.1",
        "canary.mac.reboot.2",
    ]


def test_canary_records_a_reboot_that_lost_enablement_and_then_refuses_to_end(
    tmp_path, monkeypatch
):
    workflow, fake, store = mac_workflow(tmp_path)
    item = _deployed_mac_release(tmp_path, store, workflow)
    now = _clock(monkeypatch)
    with store.exclusive():
        workflow.canary_start(item, "mac")
    fake.restart()
    fake.doctor_ok, fake.doctor_detail = False, RUNTIME_ONLY
    with store.exclusive():
        workflow.canary_reboot_observed(item, "mac")
    assert item.checkpoints["canary.mac.reboot.1"]["evidence"]["enablement_survived"] is False

    now["at"] = "2026-08-25T10:00:00Z"
    with store.exclusive(), pytest.raises(Refusal, match="not durably Wez-first"):
        workflow.canary_end(item, "mac")
    fake.doctor_ok, fake.doctor_detail = True, DURABLE_WEZ_FIRST
    with store.exclusive(), pytest.raises(Refusal, match="did not survive canary.mac.reboot.1"):
        workflow.canary_end(item, "mac")


def test_canary_end_refuses_a_stale_incarnation_named_by_doctor(tmp_path, monkeypatch):
    workflow, fake, store = mac_workflow(tmp_path)
    item = _deployed_mac_release(tmp_path, store, workflow)
    now = _clock(monkeypatch)
    with store.exclusive():
        workflow.canary_start(item, "mac")
    now["at"] = "2026-08-25T10:00:00Z"
    fake.doctor_extra = {"backend_instances": [{"backend": "wez", "state": "F"}]}
    with store.exclusive(), pytest.raises(Refusal, match=r"stale incarnation.*\['F'\]"):
        workflow.canary_end(item, "mac")
    fake.doctor_extra = {"backend_instances": [{"backend": "wez", "state": "E"}]}
    with store.exclusive():
        workflow.canary_end(item, "mac")
    assert item.checkpoints["canary.mac.end"]["evidence"]["backend_instance_states"] == ["E"]


def test_archie_canary_uses_the_archie_owner_snapshot_and_phase(tmp_path, monkeypatch):
    workflow, fake, store = mac_workflow(tmp_path)
    item = _deployed_mac_release(tmp_path, store, workflow)
    _clock(monkeypatch)
    calls = []

    def archie_owner(release, *, approved_spaces, require_quiet, **_):
        calls.append(sorted(approved_spaces))
        return {
            "host": "archie",
            "pid": 957,
            "epoch": "e",
            "backend_instance_uid": "28916e16",
            "doctor": fake.doctor_document(),
        }

    workflow._archie_owner_snapshot = archie_owner
    item.data["smoke"]["remote"] = {"space_uid": "33333333-3333-4333-8333-333333333333"}
    with store.exclusive():
        workflow._checkpoint(item, "deploy.archie.service", {"pid": 1, "epoch": "e"})
        workflow.canary_start(
            item, "archie", approved_spaces={"44444444-4444-4444-8444-444444444444"}
        )

    assert item.phase == "canary_arch"
    assert calls == [
        ["33333333-3333-4333-8333-333333333333", "44444444-4444-4444-8444-444444444444"]
    ]
    assert (
        item.checkpoints["canary.archie.start"]["evidence"]["owner"]["doctor"]["artifact"]
    ).endswith("canary.archie.start.owner-archie.json")


# Rollback rehearsal (plan §21 step 7; WS-G.2) -------------------------------


def test_every_dmux_call_states_its_policy_and_scrubs_the_ambient_one(tmp_path):
    assert "DMUX_WEZ_FIRST" in AMBIENT_MUX_VARS
    assert "DMUX_LEGACY_POLICY" in AMBIENT_MUX_VARS
    seen = []

    class Recorder(Runner):
        def capture(self, argv, **kwargs):
            seen.append((list(argv), kwargs))
            return Result(tuple(argv), 0, "{}", "")

    cfg = config(tmp_path, tmp_path / "d", tmp_path / "w")
    workflow = Workflow(RolloutStore(tmp_path / "state"), Recorder(), cfg)
    workflow._await_live_gui = lambda *a, **k: {"gui": True}

    workflow._dmux(["doctor"], env={"DMUX_LEGACY_POLICY": "1", "DMUX_DRY_RUN": "1"})
    workflow._dmux_json(["recovery", "status"])
    workflow._present_and_wait(None, host="b", name="s", host_uid="h", space_uid="u")

    assert seen[0][0] == [str(cfg.mac_dmux), "doctor"]
    assert seen[0][1]["env"] == {
        "DMUX_WEZ_FIRST": "1",
        "DMUX_LEGACY_POLICY": "1",
        "DMUX_DRY_RUN": "1",
    }
    assert seen[1][1]["env"] == {"DMUX_WEZ_FIRST": "1"}
    assert seen[2][0][1:] == [
        "con",
        "--host",
        "b",
        "--name",
        "s",
        "--backend",
        "wez",
        "--launch-gui",
    ]
    assert seen[2][1]["env"] == {"DMUX_WEZ_FIRST": "1"}
    for _, kwargs in seen:
        assert "DMUX_LEGACY_POLICY" in kwargs["unset_env"]
        assert "DMUX_WEZ_FIRST" in kwargs["unset_env"]

    remote = workflow._remote_dmux_argv(["ls", "--json"], env={"DMUX_LEGACY_POLICY": "1"})
    assert remote[-3:] == [str(cfg.archie_home / ".local/bin/dmux"), "ls", "--json"]
    assert remote.index("DMUX_LEGACY_POLICY=1") > remote.index("DMUX_LEGACY_POLICY")
    assert remote[remote.index("DMUX_LEGACY_POLICY") - 1] == "-u"
    assert "DMUX_WEZ_FIRST=1" in remote

    source = Path(Workflow.__module__.replace(".", "/") + ".py")
    text = (Path(__file__).parents[2] / "src" / source).read_text(encoding="utf-8")
    assert 'env={"DMUX_WEZ_FIRST": "1"}' not in text, "every dmux call goes through _dmux"


SMOKE_HOST = "9d1950c7-968f-4fdc-a709-0b116096598a"
SMOKE_SPACE = "01a01044-08ed-7d51-a908-9278d89238e7"


def _rehearsal_release(tmp_path, store, workflow, fake, monkeypatch):
    item = release(tmp_path)
    item.set_smoke_identity(space_uid=SMOKE_SPACE, host_uid=SMOKE_HOST)
    fake.rows.append(
        {"window_id": 1, "tab_id": 1, "pane_id": 7, "workspace": f"dmux:{SMOKE_HOST}:{SMOKE_SPACE}"}
    )
    fake.path.parent.mkdir(parents=True, exist_ok=True)
    fake.path.write_text("# canary\nDMUX_WEZ_FIRST=1\n", encoding="utf-8")
    fake.session["DMUX_WEZ_FIRST"] = "1"
    _clock(monkeypatch)
    with store.exclusive():
        store.create(item)
        item.set_phase("canary_mac")
        workflow._checkpoint(item, "canary.mac.end", {"host": "mac"})
        store.save(item)
    quits = []
    workflow._present_and_wait = lambda *a, **k: {"gui_instance": "g", "pid": 1}
    workflow._safe_quit_gui = lambda gui, **k: quits.append(gui) or {"gui_instance": "g"}
    return item, quits


def _legacy_dmux(fake, *, plan="would exec: tmux new-session -A -s rollout-smoke-rehearsal\n"):
    """Answer the rehearsal's dmux calls the way the legacy path does, and
    record the host-level policy each call saw."""
    calls = []

    def is_new(argv):
        return argv[1:2] == ["new"]

    def answer_new(argv, kwargs):
        calls.append(("new", argv[2:], kwargs["env"], dict(fake.session)))
        if "--no-connect" in argv:
            return Result(
                tuple(argv),
                2,
                "",
                "dmux: --backend/--no-connect/--allow-name-collision/--launch-gui require "
                "DMUX_WEZ_FIRST=1\n",
            )
        return Result(tuple(argv), 0, plan, "")

    def is_ls(argv):
        return argv[1:3] == ["ls", "--json"]

    def answer_ls(argv, kwargs):
        calls.append(("ls", argv[2:], kwargs["env"], dict(fake.session)))
        rows = [
            {"index": 1, "name": "rollout-smoke", "kind": "wez", "host": "macie", "windows": 1},
            {"index": 2, "name": "other", "kind": "tmux", "host": "macie", "windows": 1},
        ]
        return Result(tuple(argv), 0, json.dumps(rows), "")

    fake.responders.extend([(is_new, answer_new), (is_ls, answer_ls)])
    return calls


def test_rollback_rehearsal_flips_the_policy_proves_tmux_and_restores_the_host(
    tmp_path, monkeypatch
):
    workflow, fake, store = mac_workflow(tmp_path)
    item, quits = _rehearsal_release(tmp_path, store, workflow, fake, monkeypatch)
    calls = _legacy_dmux(fake)

    with store.exclusive():
        workflow.rollback_rehearsal(item, "mac", legacy_con_switches_gui="false")

    row = item.checkpoints["rollback.rehearsal.mac"]["evidence"]
    # (a) the policy was set the durable way and every legacy call carried it
    assert row["policy_set"]["launchd"] == {"DMUX_WEZ_FIRST": "1", "DMUX_LEGACY_POLICY": "1"}
    assert row["policy_set"]["file"]["assignments"] == {
        "DMUX_WEZ_FIRST": "1",
        "DMUX_LEGACY_POLICY": "1",
    }
    assert [kind for kind, *_ in calls] == ["new", "new", "ls"]
    for _, _, env, session in calls:
        assert env["DMUX_LEGACY_POLICY"] == "1" and env["DMUX_WEZ_FIRST"] == "1"
        assert session["DMUX_LEGACY_POLICY"] == "1", "the host carried the policy during the call"
    # (b) creation plans tmux and executes nothing; the Wez-first surface is off
    assert calls[0][1] == ["rollout-smoke-rehearsal"]
    assert calls[0][2]["DMUX_DRY_RUN"] == "1"
    assert row["legacy_create_plan"]["stdout"].startswith("would exec: tmux new-session")
    assert row["legacy_create_plan"]["backend"] == "tmux"
    assert row["legacy_create_plan"]["executed"] is False
    assert row["wez_first_surface_refused"]["returncode"] == 2
    assert "require DMUX_WEZ_FIRST=1" in row["wez_first_surface_refused"]["stderr"]
    # (c) the existing Wez Space still lists on the legacy path and presents
    assert row["legacy_listing"]["row"]["name"] == "rollout-smoke"
    assert row["legacy_listing"]["cli_env"]["DMUX_LEGACY_POLICY"] == "1"
    assert row["presentation"]["cli_env"] == {"DMUX_WEZ_FIRST": "1"}
    assert row["presentation"]["owner_panes"] == [7]
    assert quits == [{"gui_instance": "g", "pid": 1}]
    assert row["legacy_con_switches_gui"] == "false"
    # (d) the file and the session are back as the canary left them; no restart
    assert fake.path.read_text() == "# canary\nDMUX_WEZ_FIRST=1\n"
    assert fake.session == {"DMUX_WEZ_FIRST": "1"}
    assert row["restored"]["launchd"] == {"DMUX_WEZ_FIRST": "1", "DMUX_LEGACY_POLICY": ""}
    assert row["restored"]["mux_restarted"] is False
    assert not any("-k" in argv for argv in fake.sent if argv[:1] == ["launchctl"])
    assert row["after"]["pid"] == row["before"]["pid"] == fake.pid
    assert row["after"]["doctor"]["artifact"].endswith("rollback.rehearsal.mac.after-mac.json")
    assert row["before"]["doctor"]["artifact"].endswith("rollback.rehearsal.mac.before-mac.json")
    assert item.phase == "canary_mac"


def test_rollback_rehearsal_clears_the_policy_when_a_step_fails(tmp_path, monkeypatch):
    workflow, fake, store = mac_workflow(tmp_path)
    item, _ = _rehearsal_release(tmp_path, store, workflow, fake, monkeypatch)
    _legacy_dmux(fake, plan="would exec: wezterm cli spawn --workspace rollout-smoke-rehearsal\n")

    with store.exclusive(), pytest.raises(Refusal, match="did not plan a tmux session"):
        workflow.rollback_rehearsal(item, "mac")

    assert "rollback.rehearsal.mac" not in item.checkpoints
    assert fake.path.read_text() == "# canary\nDMUX_WEZ_FIRST=1\n"
    assert fake.session == {"DMUX_WEZ_FIRST": "1"}


def test_rollback_rehearsal_requires_the_canary_and_a_valid_gui_answer(tmp_path):
    workflow, fake, _store = mac_workflow(tmp_path)
    item = release(tmp_path)
    with pytest.raises(StateError, match="legacy_con_switches_gui must be one of"):
        workflow.rollback_rehearsal(item, "mac", legacy_con_switches_gui="yes")
    with pytest.raises(Refusal, match="canary.mac.end is not journaled"):
        workflow.rollback_rehearsal(item, "mac")
    assert fake.sent == []
