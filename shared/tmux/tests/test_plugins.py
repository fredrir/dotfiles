import hashlib
import importlib.machinery
import importlib.util
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / "bin/tmux-plugins"
LOADER = importlib.machinery.SourceFileLoader("tmux_plugins", str(SOURCE))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
plugins = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(plugins)


class PluginTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="tmux-plugins-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.patch(plugins, "ROOT", self.root)

    def patch(self, target, attribute, *args, **kwargs):
        mocked = patch.object(target, attribute, *args, **kwargs)
        result = mocked.start()
        self.addCleanup(mocked.stop)
        return result

    def test_download_checks_content_before_making_executable(self):
        self.patch(
            plugins.urllib.request, "urlopen", return_value=io.BytesIO(b"corrupt")
        )
        target = self.root / "download"
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            plugins.download_verified(
                plugins.LOCK["fingers"]["assets"]["Linux-x86_64"]["url"],
                "0" * 64,
                target,
            )
        self.assertFalse(os.access(target, os.X_OK))

    def test_verified_download_is_executable_and_exact(self):
        content = b"verified executable"
        self.patch(plugins.urllib.request, "urlopen", return_value=io.BytesIO(content))
        target = self.root / "download"
        plugins.download_verified(
            plugins.LOCK["fingers"]["assets"]["Linux-x86_64"]["url"],
            hashlib.sha256(content).hexdigest(),
            target,
        )
        self.assertEqual(target.read_bytes(), content)
        self.assertEqual(target.stat().st_mode & 0o777, 0o700)

    def test_failed_binary_install_does_not_publish_directory(self):
        self.patch(plugins, "fingers_binary", return_value=None)
        self.patch(plugins, "asset_key", return_value="Linux-x86_64")
        self.patch(
            plugins.urllib.request, "urlopen", return_value=io.BytesIO(b"corrupt")
        )
        executable = self.patch(plugins, "run")
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            plugins.install_fingers()
        self.assertFalse(plugins.paths()[1].parent.exists())
        executable.assert_not_called()
        self.assertEqual(list(self.root.iterdir()), [])

    def test_offline_startup_loads_installed_parts_without_network(self):
        self.patch(plugins, "fingers_binary", return_value=None)
        install_one = self.patch(plugins, "install_resurrect")
        install_two = self.patch(plugins, "install_fingers")
        load = self.patch(plugins, "load_plugins")
        self.patch(plugins, "option")
        with patch.dict(os.environ, {"DOTFILES_TMUX_OFFLINE": "1"}):
            self.assertEqual(plugins.bootstrap(), 1)
        load.assert_called_once()
        install_one.assert_not_called()
        install_two.assert_not_called()

    def test_failure_is_retained_and_retry_is_throttled(self):
        self.patch(plugins, "fingers_binary", return_value=None)
        install_one = self.patch(
            plugins, "install_resurrect", side_effect=OSError("offline")
        )
        install_two = self.patch(
            plugins, "install_fingers", side_effect=OSError("offline")
        )
        self.patch(plugins, "load_plugins")
        self.patch(plugins, "option")
        with patch.dict(os.environ, {"DOTFILES_TMUX_OFFLINE": "0"}):
            self.assertEqual(plugins.bootstrap(), 1)
            self.assertEqual(plugins.bootstrap(), 1)
        install_one.assert_called_once()
        install_two.assert_called_once()
        self.assertEqual(
            json.loads((self.root / "status.json").read_text())["errors"],
            ["offline", "offline"],
        )

    def test_float_and_multiple_clients_return_fallback_without_launch(self):
        from subprocess import CompletedProcess

        for floating, clients in (
            (b"1", b"/dev/one\n"),
            (b"0", b"/dev/one\n/dev/two\n"),
        ):
            with (
                patch.object(
                    plugins,
                    "tmux",
                    side_effect=[
                        CompletedProcess([], 0, stdout=floating),
                        CompletedProcess([], 0, stdout=clients),
                    ],
                ),
                patch.object(plugins, "run") as launch,
            ):
                self.assertEqual(plugins.fingers("%1", "/dev/one"), 3)
                launch.assert_not_called()

    def test_real_floating_pane_returns_fallback_without_launch(self):
        import re
        import shutil
        import subprocess

        binary = shutil.which("tmux")
        if not binary:
            self.skipTest("tmux is required")
        command = [binary, "-S", str(self.root / "float-test.sock")]

        def actual_tmux(*args, **kwargs):
            return subprocess.run(command + list(args), capture_output=True, check=True)

        actual_tmux(
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "float-test",
            "-x",
            "120",
            "-y",
            "40",
            "sleep 60",
        )
        self.addCleanup(
            lambda: subprocess.run(
                command + ["kill-server"], capture_output=True, check=False
            )
        )
        version = actual_tmux("display-message", "-p", "#{version}").stdout.decode()
        numbers = tuple(map(int, re.match(r"(\d+)\.(\d+)", version).groups()))
        if numbers < (3, 7):
            self.skipTest("native floating panes require tmux 3.7")
        pane = (
            actual_tmux(
                "new-pane",
                "-dP",
                "-F",
                "#{pane_id}",
                "-x",
                "60",
                "-y",
                "15",
                "sleep 60",
            )
            .stdout.decode()
            .strip()
        )
        self.patch(plugins, "tmux", side_effect=actual_tmux)
        self.patch(plugins, "fingers_binary", return_value=Path("/unused/fingers"))
        launch = self.patch(plugins, "run")
        self.assertEqual(plugins.fingers(pane, None), 3)
        launch.assert_not_called()

    def test_configuration_failure_is_visible_and_clears_after_recovery(self):
        (plugins.paths()[0] / "scripts").mkdir(parents=True)
        (plugins.paths()[0] / "scripts/save.sh").touch()
        self.patch(plugins, "fingers_binary", return_value=Path("/mock/fingers"))
        load = self.patch(
            plugins, "load_plugins", side_effect=RuntimeError("invalid style")
        )
        self.patch(plugins, "option")
        with patch("sys.stderr", new_callable=io.StringIO):
            self.assertEqual(plugins.bootstrap(), 1)
        self.assertEqual(plugins.state()["errors"], ["invalid style"])
        load.side_effect = None
        self.assertEqual(plugins.bootstrap(), 0)
        self.assertEqual(plugins.state()["errors"], [])

    def test_validator_has_no_install_or_filesystem_side_effects(self):
        load = self.patch(plugins, "load_plugins")
        with patch.dict(os.environ, {"DOTFILES_TMUX_VALIDATE": "1"}):
            self.assertEqual(plugins.bootstrap(), 0)
        load.assert_not_called()
        self.assertEqual(list(self.root.iterdir()), [])

    def test_indexed_palette_avoids_terminal_specific_ansi_slots(self):
        for background, color in (
            ("#15152b", "#8e6fcf"),
            ("#eff1f5", "#8839ef"),
            ("#15152b", "#ffffff"),
        ):
            index = plugins.indexed_color(color, background)
            self.assertGreaterEqual(index, 16)
            self.assertLessEqual(index, 255)

    def test_restore_without_snapshot_does_not_launch_upstream(self):
        from subprocess import CompletedProcess

        script = plugins.paths()[0] / "scripts/restore.sh"
        script.parent.mkdir(parents=True)
        script.touch()
        savedir = self.root / "saved"
        self.patch(
            plugins,
            "tmux",
            return_value=CompletedProcess([], 0, stdout=str(savedir).encode()),
        )
        start = self.patch(plugins.subprocess, "Popen")
        with self.assertRaisesRegex(RuntimeError, "no saved workspace"):
            plugins.recovery("restore")
        start.assert_not_called()


if __name__ == "__main__":
    unittest.main()
