from types import SimpleNamespace

import pytest

from tools.theme import profiles as profiles_module
from tools.theme.model import Theme, list_profiles, merge
from tools.theme.profiles import Selection, group_of, inventory, package_of
from tools.theme.validate import (
    _check_fonts,
    _check_palette_shape,
    validate,
)


def fake(palette=None, data=None, fonts=None, sizes=None):
    body = {"palette": palette or {}}
    body.update(data or {})
    return SimpleNamespace(
        profile="test",
        palette=body["palette"],
        data=body,
        fonts=fonts if fonts is not None else {"general": "A", "nerd": "B"},
        sizes=sizes if sizes is not None else {"terminal": 12, "terminal_mac": 13, "interface": 10},
    )


def test_merge_overrides_single_keys_inside_nested_tables():
    base = {"terminal": {"ansi": {"black": "surface1", "white": "subtext", "red": "red"}}}
    result = merge(base, {"terminal": {"ansi": {"black": "subtext"}}})
    assert result["terminal"]["ansi"] == {
        "black": "subtext",
        "white": "subtext",
        "red": "red",
    }


def test_merge_does_not_mutate_or_alias_the_base():
    base = {"eza": {"categories": {"image": "mauve"}}}
    result = merge(base, {"eza": {"categories": {"image": "pink"}}})
    assert base["eza"]["categories"]["image"] == "mauve"
    result["eza"]["categories"]["video"] = "red"
    assert "video" not in base["eza"]["categories"]


def test_merge_replaces_a_scalar_with_a_scalar():
    assert merge({"name": "Dark", "dark": True}, {"name": "Light", "dark": False}) == {
        "name": "Light",
        "dark": False,
    }


def test_duplicate_palette_colors_are_rejected():
    problems = []
    _check_palette_shape(fake(palette={"red": "#ff0000", "maroon": "#FF0000"}), problems)
    assert len(problems) == 1
    assert "duplicate colors cannot survive a profile switch" in problems[0]


def test_kde_role_shadowing_a_palette_color_is_rejected():
    problems = []
    _check_palette_shape(
        fake(palette={"overlay": "#111111"}, data={"kde": {"overlay": "overlay"}}), problems
    )
    assert any("shadows the palette color" in problem for problem in problems)


def test_font_family_with_a_comma_is_rejected():
    problems = []
    _check_fonts(fake(fonts={"general": "Noto Sans", "nerd": "Hack, Bold"}), problems)
    assert any("must not contain a comma" in problem for problem in problems)


def test_missing_font_sizes_are_all_reported_together():
    problems = []
    _check_fonts(fake(sizes={"terminal": 12}), problems)
    assert len(problems) == 2
    assert any("terminal_mac" in problem for problem in problems)
    assert any("interface" in problem for problem in problems)


def test_validate_lists_every_missing_palette_color_at_once():
    theme = Theme.load("mocha")
    del theme.palette["mauve"]
    del theme.palette["teal"]
    with pytest.raises(SystemExit) as error:
        validate(theme)
    message = str(error.value)
    assert "mauve" in message
    assert "teal" in message


def test_every_shipped_profile_is_valid():
    names = list_profiles()
    assert names
    for name in names:
        validate(Theme.load(name))


def test_group_and_package_come_from_the_output_path():
    assert group_of("shared/kitty/conf.d/fonts.conf") == "shared"
    assert package_of("shared/kitty/conf.d/fonts.conf") == "kitty"
    assert group_of("linux/kde/plasma/kdeglobals") == "linux/kde"
    assert package_of("linux/kde/plasma/kdeglobals") == "plasma"
    assert group_of("macos/fastfetch/apple.txt") == "macos"
    assert package_of("macos/fastfetch/apple.txt") == "fastfetch"


def test_resolution_prefers_package_then_group_then_shared():
    selection = Selection(
        {
            "shared": {"theme": "mocha", "obsidian": "latte"},
            "linux/kde": {"theme": "latte"},
        }
    )
    assert selection.for_path("shared/kitty/colors.conf") == "mocha"
    assert selection.for_path("shared/obsidian/themes/Fredrir/theme.css") == "latte"
    assert selection.for_path("linux/kde/plasma/kdeglobals") == "latte"
    assert selection.for_path("macos/fastfetch/apple.txt") == "mocha"


def test_a_group_without_an_entry_falls_back_to_shared():
    selection = Selection({"shared": {"theme": "latte"}})
    assert selection.for_path("linux/common/gtk/gtk-3.0/colors.css") == "latte"


def test_unknown_profile_names_the_available_ones():
    with pytest.raises(SystemExit) as error:
        Theme.load("no-such-profile")
    assert "unknown profile" in str(error.value)
    assert "mocha" in str(error.value)


def test_profiles_do_not_share_a_hex_between_different_roles():
    from tools.theme.emitters import _hex_to_name

    assert _hex_to_name(Theme.load())


def test_inventory_lists_the_packages_each_group_owns():
    owned = ["shared/kitty/colors.conf", "shared/zsh/conf.d/03-theme.zsh", "macos/fastfetch/x.txt"]
    assert inventory(owned) == {"macos": ["fastfetch"], "shared": ["kitty", "zsh"]}


@pytest.fixture
def selection_file(tmp_path, monkeypatch):
    target = tmp_path / "profiles.dotfile"
    # `_save` writes through dotfmt when it finds one, so these tests own the
    # whole PATH: the `=` column is dotfmt's answer and not this module's, and
    # a test asserting bytes has to say which of the two it is asking about.
    monkeypatch.setenv("PATH", str(tmp_path))

    def write(text):
        target.write_text(text, encoding="utf-8")
        return target

    monkeypatch.setattr(profiles_module, "SELECTION_FILE", str(target))
    return write


def test_switching_a_profile_keeps_the_surrounding_file(selection_file):
    target = selection_file("# which profile goes where\nshared {\n  theme = mocha  # base\n}\n")
    assert profiles_module.assign("shared", "theme", "latte")
    assert (
        target.read_text() == "# which profile goes where\nshared {\n  theme = latte  # base\n}\n"
    )


def test_switching_to_the_same_profile_rewrites_nothing(selection_file):
    target = selection_file("shared {\n  theme = mocha\n}\n")
    before = target.stat().st_mtime_ns
    assert not profiles_module.assign("shared", "theme", "mocha")
    assert target.stat().st_mtime_ns == before


def test_a_new_package_key_joins_the_block_at_its_indent(selection_file):
    target = selection_file("shared {\n  theme = mocha\n}\n")
    assert profiles_module.assign("shared", "obsidian", "latte")
    assert target.read_text() == "shared {\n  theme = mocha\n  obsidian = latte\n}\n"


def test_the_file_is_written_through_dotfmt(selection_file, tmp_path):
    # Where the `=` column comes from now: this file used to align it itself,
    # by a rule one space narrower than every other `.dotfile` in the tree.
    stub = tmp_path / "dotfmt"
    stub.write_text("#!/bin/sh\ncat >/dev/null\nprintf 'formatted\\n'\n")
    stub.chmod(0o755)
    target = selection_file("shared {\n  theme = mocha\n}\n")
    assert profiles_module.assign("shared", "obsidian", "latte")
    assert target.read_text() == "formatted\n"


def test_a_new_group_is_appended_as_its_own_block(selection_file):
    target = selection_file("shared {\n  theme = mocha\n}\n")
    assert profiles_module.assign("linux/kde", "theme", "latte")
    assert target.read_text() == "shared {\n  theme = mocha\n}\n\nlinux/kde {\n  theme = latte\n}\n"


def test_dropping_the_last_key_takes_the_empty_block_with_it(selection_file):
    target = selection_file("shared {\n  theme = mocha\n}\n\nlinux/kde {\n  theme = latte\n}\n")
    assert profiles_module.unassign("linux/kde", "theme")
    assert target.read_text() == "shared {\n  theme = mocha\n}\n"


def test_dropping_one_key_leaves_its_siblings_in_place(selection_file):
    target = selection_file("shared {\n  theme    = mocha\n  obsidian = latte\n}\n")
    assert profiles_module.unassign("shared", "obsidian")
    assert target.read_text() == "shared {\n  theme    = mocha\n}\n"


def test_dropping_a_key_that_is_not_there_changes_nothing(selection_file):
    selection_file("shared {\n  theme = mocha\n}\n")
    assert not profiles_module.unassign("shared", "obsidian")
    assert not profiles_module.unassign("linux/kde", "theme")


def test_overrides_exclude_the_shared_fallback():
    selection = Selection(
        {"shared": {"theme": "mocha", "obsidian": "latte"}, "macos": {"theme": "latte"}}
    )
    assert selection.overrides() == [("macos", "theme"), ("shared", "obsidian")]
