import concurrent.futures
import json
import os
import subprocess
from pathlib import Path

from .harness import wait_for


def test_shelf_roundtrip_preserves_process_and_origin(server):
    pid = server.fmt("#{pane_pid}")
    server.run("shelf-park")
    assert server.fmt("#{session_name}") == "__workspace-shelf"
    target = server.tm("display-message", "-p", "-t", "origin:", "#{pane_id}")
    server.run("shelf", "--take", server.pane, pane=target)
    assert server.fmt("#{session_name}") == "origin"
    assert server.fmt("#{pane_pid}") == pid


def test_scratch_preserves_shell_and_is_hidden_from_projects(server):
    server.run("scratch")
    panes = server.panes()
    view = next(p for p in panes if p["tool"] == "scratch-view")
    assert view["floating"]
    backing = next(p for p in panes if p["session_name"].startswith("__workspace-scratch-"))
    pid = server.fmt("#{pane_pid}", backing["id"])
    server.run("scratch")
    assert not any(p["tool"] == "scratch-view" for p in server.panes())
    server.run("scratch")
    assert server.fmt("#{pane_pid}", backing["id"]) == pid
    assert "__workspace-" not in server.run("projects", "--json").stdout


def test_concurrent_project_creation_uses_one_session(server, tmp_path):
    project = tmp_path / "concurrent"
    project.mkdir()
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        results = list(
            pool.map(lambda _: server.run("enter", project, "--detach").stdout, range(6))
        )
    assert len(set(results)) == 1
    assert server.tm("list-sessions", "-F", "#{session_name}").splitlines().count("concurrent") == 1


def test_concurrent_scratch_toggles_do_not_duplicate_backing_sessions(server):
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(lambda _: server.run("scratch"), range(4)))
    assert not any(p["tool"] == "scratch-view" for p in server.panes())
    assert (
        len([p for p in server.panes() if p["session_name"].startswith("__workspace-scratch-")])
        == 1
    )


def test_client_labels_are_exact_and_removed_on_detach(server):
    first, second = server.attach(), server.attach()
    server.run("client-update", "--from", "remote", client=first.name)
    label = server.fmt("#{E:@workspace-client-label}", client=first.name)
    assert label.startswith("remote → ")
    assert server.fmt("#{E:@workspace-client-label}", client=second.name) != label
    server.tm("detach-client", "-t", first.name)
    server.run("client-remove", client=first.name)
    assert "remote" not in server.tm("show-options", "-gqv", "@workspace-client-label")


def test_context_ignores_scratch_client(server):
    first = server.attach()
    server.run("scratch")
    wait_for(lambda: len(server.tm("list-clients").splitlines()) == 2)
    state = json.loads(server.run("inspect", "--json").stdout)
    assert state["client_name"] == first.name


def test_invalid_include_does_not_mutate_live_server(server):
    config = Path(server.env["DOTFILES_TMUX_CONFIG"])
    (config / ".tmux.conf").write_text(
        'set -g @must-not-change modified\nsource-file -F "#{@workspace_config}/broken.conf"\n'
    )
    (config / "broken.conf").write_text("set -g not-a-tmux-option true\n")
    server.tm("set-option", "-g", "@must-not-change", "original")
    result = server.run("reload", check=False)
    assert "reload validation" in result.stderr
    assert server.tm("show-options", "-gqv", "@must-not-change") == "original"


def test_projects_use_config_favorites_and_git_worktrees(server, tmp_path):
    root = tmp_path / "projects"
    project = root / "example"
    project.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(project)], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(project),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "init",
            "--allow-empty",
        ],
        check=True,
    )
    tree = tmp_path / "branch-tree"
    subprocess.run(
        ["git", "-C", str(project), "worktree", "add", "-qb", "feature", str(tree)], check=True
    )
    config = Path(server.env["DOTFILES_TMUX_CONFIG"])
    (config / "workspace.toml").write_text(
        f"[projects]\nroots = [{json.dumps(str(root))}]\nworktrees = true\nzoxide = false\n"
    )
    (config / "favorites").write_text(str(project) + "\n")
    rows = json.loads(server.run("projects", "--json").stdout)
    assert any(r["value"] == str(tree) for r in rows)
    assert any(r["value"] == str(project) and r["label"].startswith("★") for r in rows)


def test_favorites_append_once_under_concurrent_calls(server):
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(lambda _: server.run("favorite"), range(4)))
    favorites = Path(server.env["DOTFILES_TMUX_CONFIG"]) / "favorites"
    assert favorites.read_text().splitlines() == [server.env["HOME"]]


def test_scrollback_picker_jumps_to_selected_line(server, picker):
    server.attach()
    server.tm(
        "send-keys",
        "-t",
        server.pane,
        "for n in $(seq 1 100); do printf 'LINE_%03d\\n' \"$n\"; done",
        "Enter",
    )
    wait_for(lambda: "LINE_100" in server.capture())
    server.run("output", env={"TMUX_PICK_MATCH": "LINE_045"})
    assert server.fmt("#{copy_cursor_line}") == "LINE_045"


def test_palette_uses_live_notes_and_explicit_client(server, picker):
    client = server.attach()
    server.tm("new-session", "-d", "-s", "other", "/bin/sh")
    other = server.attach("other")
    server.tm(
        "bind-key",
        "-N",
        "Create targeted window",
        "-T",
        "prefix",
        "t",
        "new-window",
        "-n",
        "from-palette",
    )
    server.run("palette", client=client.name, env={"TMUX_PICK_MATCH": "Create targeted window"})
    wait_for(
        lambda: "from-palette" in server.tm("list-windows", "-t", "origin:", "-F", "#{window_name}")
    )
    assert "from-palette" not in server.tm("list-windows", "-t", "other:", "-F", "#{window_name}")
    clients = dict(
        line.split("\t")
        for line in server.tm(
            "list-clients", "-F", "#{client_name}\t#{client_session}"
        ).splitlines()
    )
    assert clients[other.name] == "other"


def test_picker_cancel_leaves_copy_mode_unchanged(server, picker):
    server.attach()
    server.tm("send-keys", "-t", server.pane, "printf 'something to search\\n'", "Enter")
    wait_for(lambda: "something to search" in server.capture())
    server.run("output", env={"TMUX_PICK_MATCH": "__cancel__"})
    assert server.fmt("#{pane_in_mode}") == "0"


def test_host_choices_come_from_inventory(server, picker, tmp_path):
    import sys

    client = server.attach()
    log = tmp_path / "ssh.json"
    ssh = Path(server.env["PATH"].split(os.pathsep)[0]) / "ssh"
    ssh.write_text(
        f'#!{sys.executable}\nimport json,sys\nopen({str(log)!r}, "w").write(json.dumps(sys.argv[1:]))\n'
    )
    ssh.chmod(0o700)
    server.run("host", client=client.name, env={"TMUX_PICK_MATCH": "second"})
    wait_for(log.exists)
    args = json.loads(log.read_text())
    assert args[-2] == "second"
    assert "exec tmux-workspace enter" in args[-1]
