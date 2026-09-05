import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

from tools.theme import contrast, registry, tmux
from tools.theme.emitters.tmux import OUTPUT, emit, render
from tools.theme.model import Theme, list_profiles, load_map, path
from tools.theme.profiles import Selection

TMUX_BINARY = os.environ.get("TMUX_BINARY") or shutil.which("tmux")


@pytest.mark.parametrize("profile", list_profiles())
def test_every_tmux_surface_and_picker_state_is_readable(profile):
    pairs = contrast.tmux_pairs(Theme.load(profile))
    assert {pair.state for pair in pairs} >= {
        "status.active",
        "status.muted",
        "status.error",
        "copy-mode-current-match-style",
        "copy-mode-selection-style",
        "pane-active-border-style",
        "popup-style",
        "menu-selected-style",
        "fzf.hl",
        "fzf.hl+",
        "fzf.marker",
    }
    assert len(pairs) == len({pair.state for pair in pairs})
    assert [pair for pair in pairs if not pair.passes] == []


@pytest.mark.parametrize("profile", list_profiles())
def test_renderer_emits_the_validated_colors_and_version_gates(profile):
    theme = Theme.load(profile)
    rendered = render(theme)
    options = dict(re.findall(r"^set -g @theme_(\w+) '([^']*)'$", rendered, re.MULTILINE))
    assert options.pop("name") == profile
    fzf = dict(item.split(":", 1) for item in options.pop("fzf_colors").split(","))
    assert options == tmux.colors(theme)
    assert all(re.fullmatch(r"#[0-9a-f]{6}", value) for value in options.values())
    for name, style in load_map("tmux")["styles"].items():
        expected = f"set -g {name} 'fg={options[style['fg']]},bg={options[style['bg']]}"
        assert expected in rendered
        if version := style.get("version"):
            assert f"%if #{{>=:#{{version}},{version}}}\n{expected}" in rendered
    for name, role in load_map("tmux")["fzf"].items():
        assert fzf[name] == options[role]
    assert "$" not in rendered
    assert "#(" not in rendered


def test_tmux_output_participates_in_package_theme_selection():
    emitter = next(emitter for emitter in registry.EMITTERS if emitter.name == "tmux")
    assert emitter.outputs() == [OUTPUT]
    assert emitter.staged
    selection = Selection({"shared": {"theme": "sexy-purple", "tmux": "latte"}})
    assert selection.for_path(OUTPUT) == "latte"

    class Captured:
        def __init__(self):
            self.files = {}

        def write(self, target, content):
            self.files[target] = content

    output = Captured()
    emit(Theme.load("latte"), output)
    assert output.files == {path(OUTPUT): render(Theme.load("latte"))}


@pytest.mark.skipif(TMUX_BINARY is None, reason="tmux unavailable")
def test_generated_theme_loads_and_reloads_in_isolated_tmux():
    with tempfile.TemporaryDirectory(prefix="dotfile-theme-tmux-") as directory:
        socket = str(Path(directory, "socket"))
        environment = {key: value for key, value in os.environ.items() if key != "TMUX"}

        def command(*args):
            result = subprocess.run(
                [TMUX_BINARY, "-S", socket, *args],
                capture_output=True,
                text=True,
                env=environment,
                check=True,
                timeout=10,
            )
            assert not result.stderr, (args, result.stderr)
            return result.stdout

        try:
            command("-f", "/dev/null", "new-session", "-d", "-s", "theme", "sleep 60")
            for profile in list_profiles():
                generated = Path(directory, f"{profile}.conf")
                generated.write_text(render(Theme.load(profile)), encoding="utf-8")
                command("source-file", str(generated))
                styles = command("show-options", "-g")
                command("source-file", str(generated))
                assert command("show-options", "-g") == styles
                assert command("show-options", "-gv", "@theme_name").strip() == profile
                assert (
                    command("show-options", "-gv", "@theme_bg").strip()
                    == tmux.colors(Theme.load(profile))["bg"]
                )
        finally:
            subprocess.run(
                [TMUX_BINARY, "-S", socket, "kill-server"],
                env=environment,
                capture_output=True,
                check=False,
                timeout=10,
            )
