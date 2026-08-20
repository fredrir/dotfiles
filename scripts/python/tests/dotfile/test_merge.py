import json
import os

import pytest

from tools.dotfile import jsonc
from tools.dotfile.merge import deep_merge


@pytest.fixture
def sandbox(tmp_path):
    repo = tmp_path / "repo"
    home = tmp_path / "home"
    (home / ".config").mkdir(parents=True)
    (repo / "shared" / "vscode").mkdir(parents=True)
    (repo / "environment" / "test").mkdir(parents=True)
    (repo / "environment" / "test" / "manifest").write_text("shared\nmacos\n")
    (repo / "config").mkdir()
    (repo / "config" / "targets.dotfile").write_text(
        "macos:shared/vscode/settings.json = ~/.config/Code/User/settings.json\n"
        "macos:shared/vscode/keybindings.json = ~/.config/Code/User/keybindings.json\n"
        "macos/vscode = ~/.config/Code/User\n"
        "linux:shared/vscode/settings.json = ~/.config/Code/User/settings.json\n"
        "linux:shared/vscode/keybindings.json = ~/.config/Code/User/keybindings.json\n"
    )
    (repo / "shared" / "vscode" / "settings.json").write_text(
        "{\n"
        "    // git\n"
        '    "git.autofetch": true,\n'
        '    "explorer.confirmDelete": false,\n'
        '    "[lua]": {\n'
        '        "editor.tabCompletion": "on",\n'
        "    },\n"
        "}\n"
    )
    (repo / "shared" / "vscode" / "keybindings.json").write_text("[]\n")
    (repo / "macos" / "vscode").mkdir(parents=True)
    (repo / "macos" / "vscode" / "settings.macos.json").write_text(
        '{\n    "shellformat.path": "/opt/homebrew/bin/shfmt"\n}\n'
    )
    env = {
        "DOTFILE_ROOT": str(repo),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "DOTFILE_PLATFORM": "macos",
    }
    return repo, home, env


def test_jsonc_strips_comments_and_trailing_commas():
    text = "{\n // note\n \"a\": 1, /* block */ \"b\": [1, 2,],\n \"c\": {\"d\": 2,},\n}\n"
    assert jsonc.loads(text) == {"a": 1, "b": [1, 2], "c": {"d": 2}}


def test_jsonc_keeps_markers_inside_strings():
    text = '{"url": "http://x//y", "tricky": "a,}", "esc": "say \\"hi\\"", "tab": "\\t"}'
    assert jsonc.loads(text) == {
        "url": "http://x//y",
        "tricky": "a,}",
        "esc": 'say "hi"',
        "tab": "\t",
    }


def test_jsonc_rejects_broken_input():
    with pytest.raises(ValueError):
        jsonc.loads("{oops")


def test_deep_merge_objects_overlay_wins_scalars():
    base = {"a": 1, "nested": {"x": 1, "y": 2}, "list": [1]}
    overlay = {"nested": {"y": 3, "z": 4}, "list": [9], "b": 2}
    assert deep_merge(base, overlay) == {
        "a": 1,
        "nested": {"x": 1, "y": 3, "z": 4},
        "list": [9],
        "b": 2,
    }


def test_link_materialises_merged_settings(tool, sandbox):
    repo, home, env = sandbox
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    merged = json.loads((home / ".config" / "Code" / "User" / "settings.json").read_text())
    assert merged == {
        "git.autofetch": True,
        "explorer.confirmDelete": False,
        "[lua]": {"editor.tabCompletion": "on"},
        "shellformat.path": "/opt/homebrew/bin/shfmt",
    }
    keybindings = home / ".config" / "Code" / "User" / "keybindings.json"
    assert os.readlink(keybindings) == str(repo / "shared" / "vscode" / "keybindings.json")
    assert not (home / ".config" / "vscode").exists()


def test_link_replaces_a_previous_symlink_with_the_merged_file(tool, sandbox):
    repo, home, env = sandbox
    settings = home / ".config" / "Code" / "User" / "settings.json"
    settings.parent.mkdir(parents=True)
    settings.symlink_to(repo / "shared" / "vscode" / "settings.json")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert not settings.is_symlink()
    assert "shellformat.path" in settings.read_text()


def test_link_blocks_on_drift_and_force_restores(tool, sandbox):
    _repo, home, env = sandbox
    settings = home / ".config" / "Code" / "User" / "settings.json"
    tool("dotfile", "link", "test", env=env)
    settings.write_text('{"edited": true}\n')
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "drifted" in result.stdout
    result = tool("dotfile", "merge", "test", "--force", env=env)
    assert result.returncode == 0
    assert "shellformat.path" in settings.read_text()


def test_status_reports_merge_state(tool, sandbox):
    _repo, home, env = sandbox
    settings = home / ".config" / "Code" / "User" / "settings.json"
    tool("dotfile", "link", "test", env=env)
    result = tool("dotfile", "status", "test", env=env)
    assert result.returncode == 0
    assert "2 linked, 0 missing, 0 differing" in result.stdout
    settings.write_text("{}\n")
    result = tool("dotfile", "status", "test", env=env)
    assert result.returncode == 1
    assert "1 linked, 0 missing, 1 differing" in result.stdout


def test_overlay_without_a_base_fails(tool, sandbox):
    repo, _home, env = sandbox
    (repo / "macos" / "vscode" / "other.macos.json").write_text("{}\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "other.json" in result.stderr


def test_package_carrying_replace_and_overlay_fails(tool, sandbox):
    repo, _home, env = sandbox
    (repo / "macos" / "vscode" / "settings.json").write_text("{}\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "both settings.json and overlay settings.macos.json" in result.stderr


def test_platform_scope_selects_the_destination(tool, sandbox):
    repo, home, env = sandbox
    (repo / "environment" / "test" / "manifest").write_text("shared\n")
    (repo / "config" / "targets.dotfile").write_text(
        "macos:shared/vscode/settings.json = ~/Library/Application Support/Code/User/settings.json\n"
        "macos:shared/vscode/keybindings.json = ~/Library/Application Support/Code/User/keybindings.json\n"
        "linux:shared/vscode/settings.json = ~/.config/Code/User/settings.json\n"
        "linux:shared/vscode/keybindings.json = ~/.config/Code/User/keybindings.json\n"
    )
    linux = dict(env, DOTFILE_PLATFORM="linux")
    result = tool("dotfile", "link", "test", env=linux)
    assert result.returncode == 0
    assert (home / ".config" / "Code" / "User" / "settings.json").exists()
    macos = dict(env, DOTFILE_PLATFORM="macos")
    result = tool("dotfile", "link", "test", env=macos)
    assert result.returncode == 0
    library = home / "Library" / "Application Support" / "Code" / "User" / "settings.json"
    assert library.exists()


def test_chained_overlays_merge_in_group_order(tool, sandbox):
    repo, home, env = sandbox
    (repo / "linux" / "common" / "vscode").mkdir(parents=True)
    (repo / "linux" / "common" / "vscode" / "settings.common.json").write_text(
        '{"fontFamily": "Hack"}\n'
    )
    (repo / "linux" / "arch" / "vscode").mkdir(parents=True)
    (repo / "linux" / "arch" / "vscode" / "settings.arch.json").write_text(
        '{"shellformat.path": "/usr/bin/shfmt"}\n'
    )
    (repo / "environment" / "test" / "manifest").write_text(
        "shared\nlinux/common\nlinux/arch\n"
    )
    (repo / "config" / "targets.dotfile").write_text(
        "linux:shared/vscode/settings.json = ~/.config/Code/User/settings.json\n"
        "linux:shared/vscode/keybindings.json = ~/.config/Code/User/keybindings.json\n"
        "linux/common/vscode = ~/.config/Code/User\n"
        "linux/arch/vscode = ~/.config/Code/User\n"
    )
    result = tool("dotfile", "link", "test", env=dict(env, DOTFILE_PLATFORM="linux"))
    assert result.returncode == 0
    merged = json.loads((home / ".config" / "Code" / "User" / "settings.json").read_text())
    assert merged["shellformat.path"] == "/usr/bin/shfmt"
    assert merged["fontFamily"] == "Hack"
    assert merged["git.autofetch"] is True


def test_unscoped_target_still_applies(tool, sandbox):
    repo, home, env = sandbox
    (repo / "config" / "targets.dotfile").write_text(
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\n"
        "shared/vscode/keybindings.json = ~/.config/Code/User/keybindings.json\n"
        "macos/vscode = ~/.config/Code/User\n"
    )
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert (home / ".config" / "Code" / "User" / "settings.json").exists()
