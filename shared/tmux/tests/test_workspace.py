import importlib.util
import os
import pty
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

BIN = Path(__file__).resolve().parents[1] / "bin"
TMUX_BINARY = os.environ.get("TMUX_TEST_BINARY", "tmux")
spec = importlib.util.spec_from_file_location("workspace", BIN / "workspace.py")
workspace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workspace)


class ParsingTests(unittest.TestCase):
    def test_host_input_rejects_options_and_commands(self):
        for target in (
            "-oProxyCommand=touch /tmp/no",
            "archie;touch",
            "$(touch /tmp/no)",
        ):
            app = workspace.Workspace(
                workspace.parser().parse_args(["host", "--", target])
            )
            with self.assertRaises(workspace.Error):
                app.host()

    def test_version_suffixes(self):
        self.assertEqual(workspace.version("tmux 3.7c"), (3, 7))
        self.assertEqual(workspace.version("3.3a"), (3, 3))


@unittest.skipUnless(shutil.which("tmux"), "tmux required")
class ServerTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="tmux-workspace-test-")
        self.root = Path(self.directory.name).resolve()
        self.sock = str(self.root / "socket")
        self.env = dict(os.environ)
        self.env.pop("TMUX", None)
        self.env.pop("TMUX_PANE", None)
        self.clients = []
        self.tmux(
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "origin",
            "-x",
            "120",
            "-y",
            "40",
            "/bin/sh",
        )
        self.tmux("set-option", "-g", "default-shell", "/bin/sh")
        self.tmux("set-option", "-g", "default-command", "/bin/sh")
        self.pane = self.tmux(
            "display-message", "-p", "-t", "origin", "#{pane_id}"
        ).stdout.strip()

    def tearDown(self):
        self.tmux("kill-server", check=False)
        for proc, fd in self.clients:
            proc.wait(timeout=3)
            os.close(fd)
        self.directory.cleanup()

    def tmux(self, *args, check=True):
        return subprocess.run(
            [TMUX_BINARY, "-S", self.sock, *args],
            text=True,
            capture_output=True,
            env=self.env,
            check=check,
        )

    def app(self, command, *extra):
        args = workspace.parser().parse_args(
            [command, "--socket", self.sock, "--pane", self.pane, *extra]
        )
        app = workspace.Workspace(args)
        app.tmux[0] = TMUX_BINARY
        return app

    def attach(self, session="origin"):
        master, slave = pty.openpty()
        proc = subprocess.Popen(
            [TMUX_BINARY, "-S", self.sock, "attach-session", "-t", session],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=dict(self.env, TERM="xterm-256color"),
        )
        os.close(slave)
        self.clients.append((proc, master))
        for _ in range(50):
            rows = self.tmux(
                "list-clients", "-F", "#{client_name}\t#{client_pid}"
            ).stdout.splitlines()
            for row in rows:
                name, pid = row.split("\t")
                if pid == str(proc.pid):
                    return name
            time.sleep(0.02)
        self.fail("client did not attach")

    def test_shelf_roundtrip_preserves_process_and_origin_session(self):
        original_pid = self.tmux(
            "display-message", "-p", "-t", self.pane, "#{pane_pid}"
        ).stdout
        self.app("shelf-park").shelf_park()
        self.assertEqual(
            self.tmux(
                "display-message", "-p", "-t", self.pane, "#{session_name}"
            ).stdout.strip(),
            workspace.SHELF,
        )
        self.assertEqual(self.tmux("has-session", "-t", "=origin").returncode, 0)
        target = self.tmux(
            "display-message", "-p", "-t", "origin:", "#{pane_id}"
        ).stdout.strip()
        app = self.app("shelf")
        app.pane = target
        app.choose = lambda rows, *args, **kwargs: rows[0]
        app.shelf()
        self.assertEqual(
            self.tmux(
                "display-message", "-p", "-t", self.pane, "#{session_name}"
            ).stdout.strip(),
            "origin",
        )
        self.assertEqual(
            self.tmux("display-message", "-p", "-t", self.pane, "#{pane_pid}").stdout,
            original_pid,
        )

    def test_project_names_with_shell_metacharacters_are_data(self):
        project = self.root / "project ; $(touch SHOULD_NOT_EXIST)"
        project.mkdir()
        app = self.app("projects")
        sid = app.project_session(project)
        self.assertEqual(app.project_session(project), sid)
        self.assertEqual(
            app.run(
                "display-message", "-p", "-t", sid, "#{session_path}"
            ).stdout.strip(),
            str(project),
        )
        self.assertFalse((self.root / "SHOULD_NOT_EXIST").exists())

    def test_real_notes_are_palette_source(self):
        self.tmux(
            "bind-key",
            "-N",
            "Find tools ; safely",
            "-T",
            "prefix",
            "Space",
            "display-message",
            "example",
        )
        rows = self.app("palette").bindings()
        matches = [r for r in rows if r.get("key") == "Space"]
        self.assertEqual(len(matches), 1)
        self.assertIn("Find tools ; safely", matches[0]["label"])

    def test_minus_binding_and_copy_mode_notes(self):
        self.tmux("bind-key", "-N", "Split below", "-T", "prefix", "-", "split-window")
        self.tmux(
            "bind-key",
            "-N",
            "Search previous prompt",
            "-T",
            "copy-mode-vi",
            "[",
            "send-keys",
            "-X",
            "previous-prompt",
        )
        rows = self.app("palette").bindings()
        self.assertTrue(
            any(row.get("key") == "-" and "Split below" in row["label"] for row in rows)
        )
        self.assertTrue(
            any(
                row.get("key") == "[" and row.get("table") == "copy-mode-vi"
                for row in rows
            )
        )

    def test_native_scratch_toggle_preserves_backing_shell(self):
        app = self.app("scratch")
        if app.server_version < (3, 7):
            self.skipTest("tmux 3.7 native floats required")
        app.scratch()
        view = next(p for p in app.panes() if p["tool"] == "scratch-view")
        self.assertEqual(view["floating"], "1")
        backing = next(
            p
            for p in app.panes()
            if p["session_name"].startswith("__workspace-scratch-")
        )
        pid = app.fmt("#{pane_pid}", pane=backing["id"])
        app.scratch()
        self.assertFalse(any(p["tool"] == "scratch-view" for p in app.panes()))
        app.scratch()
        self.assertEqual(app.fmt("#{pane_pid}", pane=backing["id"]), pid)
        self.assertFalse(
            any(s["name"].startswith("__workspace-") for s in app.sessions())
        )

    def test_client_metadata_is_exact_and_removed_on_detach(self):
        first, second = self.attach(), self.attach()
        app = self.app("client-update", "--client", first, "--from", "macie")
        app.client_update()
        expected = "macie → " + workspace.socket.gethostname().split(".")[0]
        self.assertEqual(
            app.fmt("#{E:@workspace-client-label}", client=first), expected
        )
        self.assertNotEqual(
            app.fmt("#{E:@workspace-client-label}", client=second), expected
        )
        self.tmux("detach-client", "-t", first)
        app.client_update(remove=True)
        self.assertNotIn(
            "macie", app.run("show-options", "-gqv", "@workspace-client-label").stdout
        )

    def test_copy_special_characters_roundtrip(self):
        value = "- odd # text ; $HOME \\ value"
        app = self.app("quick-select")
        app.copy(value)
        self.assertEqual(self.tmux("show-buffer").stdout, value)

    def test_context_ignores_scratch_client(self):
        client = self.attach()
        app = self.app("scratch")
        if app.server_version < (3, 7):
            self.skipTest("tmux 3.7 native floats required")
        app.scratch()
        app.client = ""
        self.assertEqual(app.context().client, client)

    def test_invalid_included_config_does_not_mutate_live_server(self):
        config = self.root / "config"
        config.mkdir()
        (config / ".tmux.conf").write_text(
            'set -g @must-not-change modified\nsource-file -F "#{@workspace_config}/broken.conf"\n'
        )
        (config / "broken.conf").write_text("set -g not-a-tmux-option true\n")
        self.tmux("set-option", "-g", "@workspace_config", str(config))
        self.tmux("set-option", "-g", "@must-not-change", "original")
        with self.assertRaises(workspace.Error):
            self.app("reload").reload()
        self.assertEqual(
            self.tmux("show-options", "-gqv", "@must-not-change").stdout.strip(),
            "original",
        )

    def test_yazi_chooser_directory_takes_precedence(self):
        result = self.root / "cwd"
        chosen = self.root / "selected directory"
        chosen.mkdir()
        app = self.app("yazi", "--cwd-file", str(result))

        def popup(argv, *args, **kwargs):
            Path(argv[argv.index("--cwd-file") + 1]).write_text(str(self.root))
            Path(argv[argv.index("--chooser-file") + 1]).write_text(str(chosen) + "\n")

        app.popup = popup
        with patch.object(workspace.shutil, "which", return_value="/usr/bin/yazi"):
            app.yazi()
        self.assertEqual(result.read_text(), str(chosen))

    def test_yazi_direct_output_supports_existing_zle_shells(self):
        app = self.app("yazi")
        app.context = lambda: app
        app.fmt = lambda expression: "zsh"
        app.run = Mock()
        with patch.object(workspace.shutil, "which", return_value="/usr/bin/yazi"):
            app.yazi()
        app.run.assert_called_once_with(
            "send-keys", "-t", self.pane, "-l", "\033[115;9u"
        )

    def test_scrollback_selection_lands_on_selected_line(self):
        # Write enough output to place the selected line into scrollback.
        self.tmux(
            "send-keys",
            "-t",
            self.pane,
            "for n in $(seq 1 100); do printf 'LINE_%03d\\n' \"$n\"; done",
            "Enter",
        )
        for _ in range(100):
            if "LINE_100" in self.tmux("capture-pane", "-p", "-t", self.pane).stdout:
                break
            time.sleep(0.01)
        app = self.app("output")
        app.choose = lambda rows, *args, **kwargs: next(
            row for row in rows if row["label"].endswith("LINE_045")
        )
        app.output()
        self.assertEqual(app.fmt("#{copy_cursor_line}"), "LINE_045")

    def test_legacy_palette_targets_origin_session_and_client(self):
        client = self.attach()
        self.tmux("new-session", "-d", "-s", "other", "/bin/sh")
        other = self.attach("other")
        self.tmux(
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
        app = self.app("palette", "--client", client)
        app._version = (3, 3)
        app.choose = lambda rows, *args, **kwargs: next(
            row for row in rows if row.get("key") == "t"
        )
        app.palette()
        windows = self.tmux(
            "list-windows", "-t", "origin:", "-F", "#{window_name}"
        ).stdout
        self.assertIn("from-palette", windows)
        self.assertNotIn(
            "from-palette",
            self.tmux("list-windows", "-t", "other:", "-F", "#{window_name}").stdout,
        )
        self.tmux(
            "bind-key",
            "-N",
            "Switch only originating client",
            "-T",
            "prefix",
            "F6",
            "switch-client",
            "-t",
            "other",
        )
        app.choose = lambda rows, *args, **kwargs: next(
            row for row in rows if row.get("key") == "F6"
        )
        app.palette()
        self.assertEqual(app.fmt("#{client_session}", client=client), "other")
        self.assertEqual(app.fmt("#{client_session}", client=other), "other")

    def test_uncertain_handoff_follow_rechecks_destination(self):
        app = self.app("agent-follow")
        app.root = lambda: self.root
        app.report = Mock()
        app.run = Mock(return_value=subprocess.CompletedProcess([], 0, "", ""))
        app.fmt = lambda expression: "$0"
        with patch.object(workspace.shutil, "which", return_value="/usr/bin/agent-hop"):
            for phase in ("moved", "commit-uncertain", "source-stopped"):
                with patch.object(
                    workspace,
                    "call",
                    return_value=subprocess.CompletedProcess(
                        [], 0, '{"phase":"' + phase + '"}', ""
                    ),
                ):
                    app.launch("agent-follow")
                launched = app.run.call_args.args
                self.assertIn("_agent-follow-client", launched)
                self.assertIn(self.pane, launched)
            with patch.object(
                workspace,
                "call",
                return_value=subprocess.CompletedProcess(
                    [], 0, '{"phase":"queued"}', ""
                ),
            ):
                app.run.reset_mock()
                app.launch("agent-follow")
                app.run.assert_not_called()
                app.report.assert_called_once()

    def test_projects_include_worktrees_without_internal_sessions(self):
        if not shutil.which("git"):
            self.skipTest("git required")
        projects = self.root / "projects"
        project = projects / "example"
        project.mkdir(parents=True)
        subprocess.run(["git", "init", "-q", project], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                project,
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
        tree = self.root / "branch-tree"
        subprocess.run(
            ["git", "-C", project, "worktree", "add", "-qb", "feature", tree],
            check=True,
        )
        app = self.app("projects")
        app.internal_session(workspace.SHELF, "shelf", self.root)
        with patch.object(workspace.Path, "home", return_value=self.root):
            rows = app.project_rows()
        self.assertTrue(any(row.get("value") == str(tree) for row in rows))
        self.assertFalse(any(workspace.SHELF in row["label"] for row in rows))


if __name__ == "__main__":
    unittest.main()
