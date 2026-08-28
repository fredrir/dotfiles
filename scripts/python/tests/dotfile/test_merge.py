import json
import os

import pytest

from tools.dotfile import jsonc, merge
from tools.dotfile.link import cmd_link
from tools.dotfile.merge import deep_merge, resolve
from tools.dotfile.state import Context, collect_groups, load_overrides
from tools.dotfile.targets import load_targets


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


@pytest.fixture
def loaded(sandbox, monkeypatch):
    """The sandbox again, plus a builder for a context with its merge entries loaded."""
    repo, home, env = sandbox
    for name, value in env.items():
        monkeypatch.setenv(name, value)

    def build():
        ctx = Context()
        load_targets(ctx)
        load_overrides(ctx)
        collect_groups(ctx, str(repo / "environment" / "test" / "manifest"), notes=False)
        merge.load(ctx)
        return ctx

    return repo, home, build


def settings_of(home):
    return home / ".config" / "Code" / "User" / "settings.json"


def reformat(path):
    """Rewrite the file the way an editor that owns it would: same document, own layout."""
    text = json.dumps(json.loads(path.read_text()), indent=2) + "\n"
    path.write_text(text)
    return text


def outcome(ours, theirs, base, ignores=(), decisions=None):
    document, changes = resolve(ours, theirs, base, list(ignores), decisions)
    return document, {change.key(): change.kind for change in changes}


def test_jsonc_strips_comments_and_trailing_commas():
    text = '{\n // note\n "a": 1, /* block */ "b": [1, 2,],\n "c": {"d": 2,},\n}\n'
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
    result = tool("dotfile", "link", "test", "--force", env=env)
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


def test_three_way_applies_a_key_the_repo_added():
    document, kinds = outcome({"a": 1, "b": 2}, {"a": 1}, {"a": 1})
    assert document == {"a": 1, "b": 2}
    assert kinds == {}


def test_three_way_flags_a_key_deleted_on_this_machine():
    document, kinds = outcome({"a": 1, "b": 2}, {"a": 1}, {"a": 1, "b": 2})
    assert kinds == {"b": "delete"}
    assert document == {"a": 1}


def test_three_way_flags_a_key_added_on_this_machine():
    document, kinds = outcome({"a": 1}, {"a": 1, "b": 2}, {"a": 1})
    assert kinds == {"b": "add"}
    assert document == {"a": 1, "b": 2}


def test_three_way_drops_a_key_the_repo_deleted():
    document, kinds = outcome({"a": 1}, {"a": 1, "b": 2}, {"a": 1, "b": 2})
    assert kinds == {}
    assert document == {"a": 1}


def test_three_way_is_quiet_when_only_the_repo_moved():
    document, kinds = outcome({"a": 2}, {"a": 1}, {"a": 1})
    assert kinds == {}
    assert document == {"a": 2}


def test_three_way_flags_a_key_only_this_machine_moved():
    document, kinds = outcome({"a": 1}, {"a": 2}, {"a": 1})
    assert kinds == {"a": "modify"}
    assert document == {"a": 2}


def test_three_way_conflicts_when_both_sides_moved():
    document, kinds = outcome({"a": 2}, {"a": 3}, {"a": 1})
    assert kinds == {"a": "conflict"}
    assert document == {"a": 3}


def test_three_way_says_nothing_when_the_sides_agree():
    document, kinds = outcome({"a": 1}, {"a": 1}, {"a": 0})
    assert kinds == {}
    assert document == {"a": 1}


def test_three_way_walks_into_nested_objects():
    ours = {"[lua]": {"editor.tabSize": 4, "editor.formatOnSave": True}}
    theirs = {"[lua]": {"editor.tabSize": 2, "editor.formatOnSave": True}}
    document, kinds = outcome(ours, theirs, ours)
    assert kinds == {"[lua]/editor.tabSize": "modify"}
    assert document == theirs


def test_without_a_baseline_every_difference_is_a_change():
    document, kinds = outcome({"a": 1, "b": 2}, {"a": 9, "c": 3}, None)
    assert kinds == {"a": "modify", "b": "add", "c": "add"}
    assert document == {"a": 9, "c": 3}


def test_an_empty_baseline_is_still_a_baseline():
    document, kinds = outcome({"a": 1}, {"b": 2}, {})
    assert kinds == {"b": "add"}
    assert document == {"a": 1, "b": 2}


def test_a_repo_decision_takes_the_repo_value():
    document, changes = resolve({"a": 1}, {"a": 2}, {"a": 1}, [], {("a",): merge.REPO})
    assert document == {"a": 1}
    assert [change.kind for change in changes] == ["modify"]


def test_a_repo_decision_restores_a_key_deleted_on_this_machine():
    document, _changes = resolve({"a": 1}, {}, {"a": 1}, [], {("a",): merge.REPO})
    assert document == {"a": 1}


def test_a_repo_decision_drops_a_key_added_on_this_machine():
    document, _changes = resolve({"a": 1}, {"a": 1, "b": 2}, {"a": 1}, [], {("b",): merge.REPO})
    assert document == {"a": 1}


def test_a_subtree_the_repo_deleted_leaves_no_empty_object():
    ours = {"a": 1}
    live = {"a": 1, "[lua]": {"editor.tabSize": 2}}
    document, kinds = outcome(ours, live, live)
    assert document == {"a": 1}
    assert kinds == {}


def test_an_object_the_repo_wants_empty_stays_empty():
    document, kinds = outcome({"[lua]": {}}, {}, {})
    assert document == {"[lua]": {}}
    assert kinds == {}


def test_an_ignored_key_keeps_the_live_value_and_is_not_a_change():
    ours = {"workbench.colorTheme": "Dark"}
    theirs = {"workbench.colorTheme": "Solarized"}
    document, kinds = outcome(ours, theirs, ours, ["workbench.colorTheme"])
    assert document == theirs
    assert kinds == {}


def test_an_ignored_key_only_this_machine_has_survives():
    ours = {"a": 1}
    theirs = {"a": 1, "cSpell.userWords": ["kubectl"]}
    document, kinds = outcome(ours, theirs, ours, ["cSpell.*"])
    assert document == theirs
    assert kinds == {}


def test_an_ignored_key_the_live_file_lacks_is_omitted():
    document, kinds = outcome({"a": 1, "theme": "Dark"}, {"a": 1}, {"a": 1}, ["theme"])
    assert document == {"a": 1}
    assert kinds == {}


def test_an_ignored_nested_key_is_left_alone():
    ours = {"[lua]": {"editor.tabSize": 4, "editor.formatOnSave": True}}
    theirs = {"[lua]": {"editor.tabSize": 2}}
    base = {"[lua]": {"editor.tabSize": 4}}
    document, kinds = outcome(ours, theirs, base, ["[lua]/editor.tabSize"])
    assert document == {"[lua]": {"editor.tabSize": 2, "editor.formatOnSave": True}}
    assert kinds == {}


def test_a_reformatted_file_is_left_exactly_as_it_is(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    text = reformat(settings)
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert "(formatting)" in result.stdout
    assert settings.read_text() == text


def test_status_counts_a_reformatted_file_as_linked(tool, sandbox):
    _repo, home, env = sandbox
    tool("dotfile", "link", "test", env=env)
    reformat(settings_of(home))
    result = tool("dotfile", "status", "test", env=env)
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
    assert "(wrote)" in result.stdout
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
    assert "(drifted)" in result.stdout
    assert "modify: git.autofetch" in result.stdout
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
    assert "(conflict)" in result.stdout
    assert "conflict: git.autofetch" in result.stdout


def test_a_live_only_key_drifts_without_an_ignore(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    tool("dotfile", "link", "test", env=env)
    live = json.loads(settings.read_text())
    live["cSpell.userWords"] = ["kubectl"]
    settings.write_text(json.dumps(live, indent=2) + "\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "add: cSpell.userWords" in result.stdout


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
    result = tool("dotfile", "status", "test", env=env)
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
    assert "(drifted)" in result.stdout
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
    assert "would merge" in result.stdout
    assert not settings_of(home).exists()
    assert not (home / ".config" / "dotfile" / "merge").exists()


def test_link_force_discards_local_edits(loaded, monkeypatch):
    _repo, home, _build = loaded
    settings = settings_of(home)
    cmd_link(Context(), "test", False, [])
    settings.write_text('{"edited": true}\n')
    with pytest.raises(SystemExit):
        cmd_link(Context(), "test", False, [])
    assert settings.read_text() == '{"edited": true}\n'
    cmd_link(Context(), "test", False, [], force=True)
    assert "shellformat.path" in settings.read_text()


def test_live_resolution_keeps_the_local_value_and_records_the_adoption(loaded):
    _repo, home, build = loaded
    settings = settings_of(home)
    merge.apply_entries(build(), False)
    live = json.loads(settings.read_text())
    live["git.autofetch"] = False
    settings.write_text(json.dumps(live, indent=2) + "\n")
    ctx = build()
    assert merge.apply_entries(ctx, False, resolution=merge.LIVE) is False
    assert json.loads(settings.read_text())["git.autofetch"] is False
    entry, changes = ctx.merge_adopted[0]
    assert entry.dst == str(settings)
    assert [(change.key(), change.kind) for change in changes] == [("git.autofetch", "modify")]


def test_a_resolver_decides_key_by_key(loaded):
    _repo, home, build = loaded
    settings = settings_of(home)
    merge.apply_entries(build(), False)
    live = json.loads(settings.read_text())
    live["git.autofetch"] = False
    live["editor.fontSize"] = 13
    settings.write_text(json.dumps(live, indent=2) + "\n")
    seen = []

    def partial(_ctx, _entry, changes):
        seen.extend(change.key() for change in changes)
        return {change.path: merge.REPO for change in changes if change.kind == "modify"}

    assert merge.apply_entries(build(), False, resolver=partial) is True
    assert sorted(seen) == ["editor.fontSize", "git.autofetch"]
    assert json.loads(settings.read_text())["git.autofetch"] is False

    def everything(_ctx, _entry, changes):
        return {change.path: merge.REPO for change in changes}

    assert merge.apply_entries(build(), False, resolver=everything) is False
    merged = json.loads(settings.read_text())
    assert merged["git.autofetch"] is True
    assert "editor.fontSize" not in merged


def test_a_directory_at_the_destination_is_never_cleared_away(tool, sandbox):
    _repo, home, env = sandbox
    settings = settings_of(home)
    settings.mkdir(parents=True)
    (settings / "keep").write_text("mine\n")
    for args in (("link", "test"), ("link", "test", "--force")):
        result = tool("dotfile", *args, env=env)
        assert result.returncode == 1
        assert "(drifted)" in result.stdout
    assert (settings / "keep").read_text() == "mine\n"


def test_link_passes_force_to_both_subsystems(loaded, monkeypatch):
    seen = []

    def spy(_ctx, _dry, force, _quiet):
        seen.append(force)
        return False

    monkeypatch.setattr("tools.dotfile.link.run_apply", spy)
    cmd_link(Context(), "test", False, [])
    cmd_link(Context(), "test", False, [], force=True)
    assert seen == [False, True]
