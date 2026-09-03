from types import SimpleNamespace

import pytest

from tools.theme import oklab
from tools.theme import profiles as profiles_module
from tools.theme.model import Theme, list_profiles, load_toml, profile_file
from tools.theme.profiles import Selection, group_of, inventory, package_of
from tools.theme.schema import ANSI_KEYS, UI_KEYS
from tools.theme.validate import _check_contrast, _check_fonts, validate, validate_all


def fake(data=None, fonts=None, sizes=None):
    return SimpleNamespace(
        profile="test",
        data=data or {},
        fonts=fonts if fonts is not None else {"general": "A", "nerd": "B"},
        sizes=sizes if sizes is not None else {"terminal": 12, "terminal_mac": 13, "interface": 10},
    )


def test_a_bright_matching_its_normal_is_allowed():
    theme = Theme.load("sexy-purple")
    assert theme.hex("red") == theme.hex("bright_red")
    validate(theme)


def test_semantics_resolve_into_the_palette_at_load():
    theme = Theme.load("mocha")
    assert theme.hex("surface") == theme.hex("ui.surface")
    assert theme.hex("sidebar") == theme.hex("ui.accent")
    assert theme.hex("sunken") == theme.hex("background/-100")


def test_profiles_have_only_the_canonical_schema():
    for name in list_profiles():
        raw = load_toml(profile_file(name))
        assert set(raw) == {"name", "dark", "ui", "ansi"}
        assert set(raw["ui"]) == set(UI_KEYS)
        assert set(raw["ansi"]) == {"normal", "bright"}
        assert set(raw["ansi"]["normal"]) == set(ANSI_KEYS)
        assert set(raw["ansi"]["bright"]) == set(ANSI_KEYS)


def test_no_surface_alt_semantic_exists():
    with pytest.raises(SystemExit):
        Theme.load("sexy-purple").hex("surface_alt")


def test_the_ladder_steps_away_from_the_background_in_both_directions():
    dark = Theme.load("mocha")
    light = Theme.load("latte")
    assert oklab.relative_luminance(dark.hex("sunken")) < oklab.relative_luminance(
        dark.hex("background")
    )
    assert oklab.relative_luminance(light.hex("sunken")) < oklab.relative_luminance(
        light.hex("background")
    )


def test_contextual_contrast_pairs_are_hard_requirements():
    theme = Theme.load("latte")
    problems = []
    _check_contrast(theme, problems)
    assert problems == []
    assert oklab.contrast_ratio(theme.hex("on_primary"), theme.hex("primary_fill")) >= 4.5


def test_font_family_with_a_comma_is_rejected():
    problems = []
    _check_fonts(fake(fonts={"general": "Noto Sans", "nerd": "Hack, Bold"}), problems)
    assert any("must not contain a comma" in problem for problem in problems)


def test_a_missing_font_size_is_reported():
    problems = []
    _check_fonts(fake(sizes={"terminal": 12}), problems)
    assert len(problems) == 1
    assert any("interface" in problem for problem in problems)


def test_validate_lists_every_missing_palette_color_at_once():
    theme = Theme.load("mocha")
    del theme.semantic["magenta"]
    del theme.semantic["cyan"]
    del theme.palette["magenta"]
    del theme.palette["cyan"]
    with pytest.raises(SystemExit) as error:
        validate(theme)
    message = str(error.value)
    assert "magenta" in message
    assert "cyan" in message


def test_every_shipped_profile_is_valid():
    names = list_profiles()
    assert names
    for name in names:
        validate(Theme.load(name))
    assert len(validate_all()) == len(names)


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


def test_hex_remapping_lets_the_active_profile_name_a_shared_hex():
    from tools.theme.emitters import _hex_to_name

    mapping = _hex_to_name(Theme.load("mocha"))
    assert mapping["1e1e2e"] == "background"
    assert mapping["f38ba8"] == "red"
    assert mapping["bf616a"] == "red"
    assert mapping["6c7086"] == "separator"


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
