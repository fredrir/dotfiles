import json
import os

import pytest


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
        "SYSINFO_HOST": "test",
    }
    return repo, home, env


def settings_of(home):
    return home / ".config" / "Code" / "User" / "settings.json"


def reformat(path):
    """Rewrite the file the way an editor that owns it would: same document, own layout."""
    text = json.dumps(json.loads(path.read_text()), indent=2) + "\n"
    path.write_text(text)
    return text


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
    result = tool("dotfile", "link", "test", "--force", env=env)
    assert result.returncode == 0
    assert "shellformat.path" in settings.read_text()


def test_doctor_reports_merge_state(tool, sandbox):
    _repo, home, env = sandbox
    settings = home / ".config" / "Code" / "User" / "settings.json"
    tool("dotfile", "link", "test", env=env)
    result = tool("dotfile", "doctor", "test", env=env)
    assert result.returncode == 0
    assert "2 linked, 0 missing, 0 differing" in result.stdout
    settings.write_text("{}\n")
    result = tool("dotfile", "doctor", "test", env=env)
    assert result.returncode == 1
    assert "1 linked, 0 missing, 1 differing" in result.stdout
    assert "drifted" in result.stdout
    assert "shellformat.path" in result.stdout


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
    (repo / "environment" / "test" / "manifest").write_text("shared\nlinux/common\nlinux/arch\n")
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


def test_a_reformatted_file_is_left_exactly_as_it_is(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    text = reformat(settings)
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert "formatting preserved" in result.stdout
    assert settings.read_text() == text


def test_doctor_counts_a_reformatted_file_as_linked(tool, sandbox):
    _repo, home, env = sandbox
    tool("dotfile", "link", "test", env=env)
    reformat(settings_of(home))
    result = tool("dotfile", "doctor", "test", env=env)
    assert result.returncode == 0
    assert "formatting" in result.stdout
    assert "2 linked, 0 missing, 0 differing" in result.stdout


def test_a_repo_change_lands_without_a_decision(tool, sandbox):
    repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    reformat(settings)
    (repo / "shared" / "vscode" / "settings.json").write_text(
        "{\n"
        '    "git.autofetch": false,\n'
        '    "explorer.confirmDelete": false,\n'
        '    "[lua]": {"editor.tabCompletion": "on"},\n'
        '    "editor.fontSize": 13\n'
        "}\n"
    )
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert "updated from repository" in result.stdout
    merged = json.loads(settings.read_text())
    assert merged["git.autofetch"] is False
    assert merged["editor.fontSize"] == 13
    assert merged["shellformat.path"] == "/opt/homebrew/bin/shfmt"


def test_a_local_edit_blocks_and_is_left_untouched(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    live = json.loads(settings.read_text())
    live["git.autofetch"] = False
    settings.write_text(json.dumps(live, indent=2) + "\n")
    text = settings.read_text()
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "drifted" in result.stdout
    assert "modify:git.autofetch" in result.stdout
    assert settings.read_text() == text


def test_a_key_both_sides_moved_is_a_conflict(tool, sandbox):
    repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    live = json.loads(settings.read_text())
    live["git.autofetch"] = False
    settings.write_text(json.dumps(live, indent=2) + "\n")
    (repo / "shared" / "vscode" / "settings.json").write_text('{"git.autofetch": "daily"}\n')
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "conflict:git.autofetch" in result.stdout
    assert "conflict:git.autofetch" in result.stdout


def test_a_live_only_key_drifts_without_an_ignore(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    live = json.loads(settings.read_text())
    live["cSpell.userWords"] = ["kubectl"]
    settings.write_text(json.dumps(live, indent=2) + "\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "add:cSpell.userWords" in result.stdout


def test_ignored_keys_pass_through_a_sync(tool, sandbox):
    repo, home, env = sandbox
    (repo / "shared" / "vscode" / "merge.dotfile").write_text(
        "# this machine owns its spelling list\nignore cSpell.*\n"
    )
    (repo / "macos" / "vscode" / "merge.dotfile").write_text("ignore workbench.colorTheme\n")
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    live = json.loads(settings.read_text())
    live["cSpell.userWords"] = ["kubectl"]
    live["workbench.colorTheme"] = "Solarized"
    settings.write_text(json.dumps(live, indent=2) + "\n")
    (repo / "shared" / "vscode" / "settings.json").write_text(
        '{"git.autofetch": true, "editor.fontSize": 13}\n'
    )
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    merged = json.loads(settings.read_text())
    assert merged["cSpell.userWords"] == ["kubectl"]
    assert merged["workbench.colorTheme"] == "Solarized"
    assert merged["editor.fontSize"] == 13


def test_merge_dotfile_is_never_linked(tool, sandbox):
    repo, home, env = sandbox
    (repo / "shared" / "vscode" / "merge.dotfile").write_text("ignore cSpell.*\n")
    (repo / "macos" / "vscode" / "merge.dotfile").write_text("ignore workbench.colorTheme\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert not os.path.lexists(home / ".config" / "Code" / "User" / "merge.dotfile")
    assert not os.path.lexists(home / ".config" / "vscode")
    result = tool("dotfile", "doctor", "test", env=env)
    assert result.returncode == 0
    assert "2 linked, 0 missing, 0 differing" in result.stdout


def test_force_replaces_a_foreign_symlink_at_the_destination(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    outside = home / "elsewhere.json"
    outside.write_text('{"outside": true}\n')
    settings.parent.mkdir(parents=True)
    settings.symlink_to(outside)
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "foreign symlink where a merged file belongs" in result.stdout
    assert settings.is_symlink()
    result = tool("dotfile", "link", "test", "--force", env=env)
    assert result.returncode == 0
    assert not settings.is_symlink()
    assert json.loads(settings.read_text())["shellformat.path"] == "/opt/homebrew/bin/shfmt"
    assert json.loads(outside.read_text()) == {"outside": True}


def test_an_unparseable_destination_needs_force(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    settings.parent.mkdir(parents=True)
    settings.write_text("{ not json\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert settings.read_text() == "{ not json\n"
    result = tool("dotfile", "link", "test", "--force", env=env)
    assert result.returncode == 0
    assert "git.autofetch" in settings.read_text()


def test_a_dry_run_materialises_nothing(tool, sandbox):
    _repo, home, env = sandbox
    result = tool("dotfile", "link", "test", "-n", env=env)
    assert result.returncode == 0
    assert "would materialize" in result.stdout
    assert not settings_of(home).exists()
    assert not (home / ".config" / "dotfile" / "merge").exists()


def test_a_directory_at_the_destination_is_never_cleared_away(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    settings.mkdir(parents=True)
    (settings / "keep").write_text("mine\n")
    for args in (("link", "test"), ("link", "test", "--force")):
        result = tool("dotfile", *args, env=env)
        assert result.returncode == 1
        assert "directory where a merged file belongs" in result.stdout
    assert (settings / "keep").read_text() == "mine\n"
