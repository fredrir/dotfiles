from types import SimpleNamespace

import pytest

from tools.theme.model import Theme, list_profiles, merge, read_active
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
    _check_palette_shape(fake(palette={"overlay": "#111111"}, data={"kde": {"overlay": "overlay"}}), problems)
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


def test_load_without_a_profile_uses_the_active_one():
    assert Theme.load().profile == read_active()


def test_unknown_profile_names_the_available_ones():
    with pytest.raises(SystemExit) as error:
        Theme.load("no-such-profile")
    assert "unknown profile" in str(error.value)
    assert "mocha" in str(error.value)


def test_profiles_do_not_share_a_hex_between_different_roles():
    from tools.theme.emitters import _hex_to_name

    assert _hex_to_name(Theme.load())
