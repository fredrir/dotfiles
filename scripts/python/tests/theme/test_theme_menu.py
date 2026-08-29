import pytest

from tools.core.menu import Pick
from tools.theme import cli
from tools.theme.profiles import DEFAULT_GROUP, THEME_KEY, Selection

PROFILES = ["latte", "mocha"]
GROUPS = {
    "linux/arch": ["fastfetch"],
    "linux/kde": ["panel-colorizer", "plasma"],
    "shared": ["nvim", "zsh"],
}


@pytest.fixture(autouse=True)
def profiles(monkeypatch):
    monkeypatch.setattr(cli, "list_profiles", lambda: list(PROFILES))
    monkeypatch.setattr(cli, "_describe", lambda name: f"Catppuccin {name}")


@pytest.fixture
def selection():
    return Selection({DEFAULT_GROUP: {THEME_KEY: "mocha"}, "linux/kde": {"plasma": "latte"}})


def walk(expand, options):
    picks = ()
    for option in options:
        column = expand(picks)
        picks += (Pick(column.kind, column.options.index(option), option),)
    return picks


def test_the_root_column_is_the_menu(selection):
    column = cli._expand(selection, GROUPS)(())
    assert column.kind == "menu"
    assert column.options == list(cli.MENU)
    assert column.details == list(cli.MENU_HELP)


def test_a_switch_flow_opens_at_the_scope_column(selection):
    column = cli._expand(selection, GROUPS, flow="switch")(())
    assert column.kind == "scope"
    assert column.options == [cli.EVERYTHING, *GROUPS]


def test_the_scope_column_reports_each_group_resolution(selection):
    column = cli._expand(selection, GROUPS)(walk(cli._expand(selection, GROUPS), ["switch"]))
    assert column.details[0] == "mocha"
    assert column.details[column.options.index("linux/kde")] == "mocha   panel-colorizer, plasma"


@pytest.mark.parametrize(
    ("options", "kind"),
    [
        (["sync"], None),
        (["status"], None),
        (["dry"], None),
        (["preview"], "profile"),
        (["preview", "mocha"], None),
        (["switch"], "scope"),
        (["switch", "global"], "profile"),
        (["switch", "global", "mocha"], None),
        (["switch", "linux/arch"], "profile"),
        (["switch", "linux/kde"], "package"),
        (["switch", "linux/kde", "plasma"], "profile"),
        (["switch", "linux/kde", "plasma", "latte"], None),
    ],
)
def test_each_path_opens_the_column_it_should(selection, options, kind):
    expand = cli._expand(selection, GROUPS)
    column = expand(walk(expand, options))
    assert (column.kind if column else None) == kind


def test_a_group_with_one_package_skips_the_package_column(selection):
    expand = cli._expand(selection, GROUPS)
    assert expand(walk(expand, ["switch", "linux/arch"])).kind == "profile"


def test_the_package_column_shows_what_each_package_resolves_to(selection):
    expand = cli._expand(selection, GROUPS)
    column = expand(walk(expand, ["switch", "linux/kde"]))
    assert column.options == [cli.WHOLE_GROUP, "panel-colorizer", "plasma"]
    assert column.details == ["every file in linux/kde, now mocha", "mocha", "latte"]


def test_the_profile_column_starts_on_the_profile_in_use(selection):
    expand = cli._expand(selection, GROUPS)
    column = expand(walk(expand, ["switch", "linux/kde", "plasma"]))
    assert column.options == PROFILES
    assert column.options[column.index] == "latte"


def test_no_profiles_becomes_a_note_rather_than_an_exit(selection, monkeypatch):
    monkeypatch.setattr(cli, "list_profiles", list)
    expand = cli._expand(selection, GROUPS)
    column = expand(walk(expand, ["switch", "global"]))
    assert column.kind == "note"
    assert column.options == ["no profiles in theme/profiles"]


@pytest.mark.parametrize(
    ("options", "scope"),
    [
        ([], (DEFAULT_GROUP, THEME_KEY, False)),
        (["switch", "global"], (DEFAULT_GROUP, THEME_KEY, True)),
        (["switch", "linux/arch"], ("linux/arch", THEME_KEY, False)),
        (["switch", "linux/kde", cli.WHOLE_GROUP], ("linux/kde", THEME_KEY, False)),
        (["switch", "linux/kde", "plasma"], ("linux/kde", "plasma", False)),
    ],
)
def test_the_scope_is_rebuilt_from_the_picks(selection, options, scope):
    assert cli._scope_of(walk(cli._expand(selection, GROUPS), options)) == scope


def test_a_group_named_like_a_head_row_is_read_by_name_not_position(selection):
    groups = {cli.EVERYTHING: ["nvim"], **GROUPS}
    picks = walk(cli._expand(selection, groups), ["switch", cli.EVERYTHING])
    assert picks[-1].index == 0
    assert cli._scope_of(picks) == (DEFAULT_GROUP, THEME_KEY, True)
