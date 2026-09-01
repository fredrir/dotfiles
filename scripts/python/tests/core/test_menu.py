import io
import os
import re

import pytest

from tools.core import menu
from tools.core.menu import Column

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
JUMP = re.compile(r"\x1b\[(\d+)A")

UP = "\x1b[A"
DOWN = "\x1b[B"
RIGHT = "\x1b[C"
LEFT = "\x1b[D"
ENTER = "\r"

MENU = ["sync", "switch", "status", "preview", "dry"]
SCOPES = ["global", "linux/arch", "linux/common"]
DETAILS = ["mocha", "mocha   fastfetch", "mocha   gtk, quicklaunch"]


class Screen(io.StringIO):
    """A stdout stand-in that can claim to be a terminal."""

    def __init__(self, terminal=False):
        super().__init__()
        self.terminal = terminal

    def isatty(self):
        return self.terminal


@pytest.fixture(autouse=True)
def terminal(monkeypatch):
    monkeypatch.setenv("COLUMNS", "100")
    monkeypatch.setenv("LINES", "40")
    monkeypatch.delenv("NO_COLOR", raising=False)


def screen(lines):
    return lambda: os.terminal_size((80, lines))


def plain(text):
    return ANSI.sub("", text)


def frames(sheet):
    chunks = JUMP.split(sheet.getvalue())[::2]
    painted = [plain(chunk).split("\n")[:-1] for chunk in chunks]
    return [frame for frame in painted if frame]


def tail(sheet):
    return plain(sheet.getvalue().rpartition(menu.ERASE)[2])


def two_levels(picks):
    if not picks:
        return Column(MENU, kind="menu")
    if picks[-1].kind == "menu" and picks[-1].option == "switch":
        return Column(SCOPES, DETAILS, kind="scope")
    return None


def run(expand, keys, title="dotfile theme", start=()):
    sheet = Screen()
    picks = menu.cascade(title, expand, start=start, keys=keys, out=sheet)
    return picks, sheet


def rows(frame):
    return frame[4:]


def test_pick_needs_a_terminal(monkeypatch):
    monkeypatch.setattr(menu.sys, "stdout", Screen(False))
    assert menu.pick("pick one", ["a", "b"]) is None


def test_pick_needs_options(monkeypatch):
    monkeypatch.setattr(menu.sys, "stdout", Screen(True))
    assert menu.pick("pick one", []) is None


def test_a_single_column_frame_reads_like_a_plain_list():
    _picks, sheet = run(lambda picks: None if picks else Column(MENU, kind="menu"), ["q"])
    assert frames(sheet)[0] == [
        "",
        "  dotfile theme",
        "  ↑/↓ move | ↩ select | q quit",
        "",
        "  ❯ sync",
        "    switch",
        "    status",
        "    preview",
        "    dry",
    ]


def test_a_child_column_opens_beside_its_parent():
    _picks, sheet = run(two_levels, [DOWN, ENTER, DOWN, "q"])
    assert rows(frames(sheet)[-1]) == [
        "    sync",
        "  ❯ switch       global        mocha",
        "    status     ❯ linux/arch    mocha   fastfetch",
        "    preview      linux/common  mocha   gtk, quicklaunch",
        "    dry",
    ]


def test_only_the_active_column_shows_details():
    def expand(picks):
        column = two_levels(picks)
        if column is None and picks[-1].kind == "scope":
            return Column(["group", "plasma"], ["every file", "mocha"], kind="package")
        return column

    _picks, sheet = run(expand, [DOWN, ENTER, DOWN, DOWN, ENTER, "q"])
    body = rows(frames(sheet)[-1])
    assert "mocha   gtk, quicklaunch" not in "\n".join(body)
    assert body == [
        "    sync",
        "  ❯ switch       global",
        "    status       linux/arch",
        "    preview    ❯ linux/common    ❯ group   every file",
        "    dry                            plasma  mocha",
    ]


def test_the_hint_gains_a_level_key_once_a_column_is_open():
    _picks, sheet = run(two_levels, [DOWN, ENTER, "q"])
    assert frames(sheet)[0][2] == "  ↑/↓ move | ↩ select | q quit"
    assert frames(sheet)[-1][2] == "  ↑/↓ move | ←/→ level | ↩ select | q quit"


def test_left_pops_a_column_and_restores_the_parent_cursor():
    _picks, sheet = run(two_levels, [DOWN, ENTER, DOWN, LEFT, "q"])
    painted = frames(sheet)
    assert rows(painted[-1]) == rows(painted[1])
    assert painted[-1][2] == "  ↑/↓ move | ↩ select | q quit"


def test_left_at_the_first_column_does_nothing():
    _picks, sheet = run(two_levels, [LEFT, "q"])
    painted = frames(sheet)
    assert painted[0] == painted[-1]


def test_right_on_a_leaf_does_not_select():
    picks, sheet = run(two_levels, [RIGHT, "q"])
    assert picks is None
    assert frames(sheet)[0] == frames(sheet)[-1]


def test_enter_on_a_leaf_returns_the_whole_path():
    picks, _sheet = run(two_levels, [DOWN, ENTER, DOWN, ENTER])
    assert picks == [
        menu.Pick("menu", 1, "switch"),
        menu.Pick("scope", 1, "linux/arch"),
    ]


def test_quitting_deep_returns_nothing():
    picks, _sheet = run(two_levels, [DOWN, ENTER, DOWN, "q"])
    assert picks is None


def test_a_finished_cascade_collapses_to_one_line():
    _picks, sheet = run(two_levels, [DOWN, ENTER, DOWN, ENTER])
    assert tail(sheet).splitlines() == ["  dotfile theme — switch › linux/arch"]


def test_an_abandoned_cascade_leaves_nothing_behind():
    _picks, sheet = run(two_levels, [DOWN, ENTER, "q"])
    assert tail(sheet) == ""


def test_expand_is_consulted_once_per_path():
    calls = []

    def counted(picks):
        calls.append(picks)
        return two_levels(picks)

    run(counted, [DOWN, ENTER, DOWN, UP, LEFT, ENTER, "q"])
    assert len(calls) == len(set(calls))


def test_vi_keys_match_the_arrows():
    _arrows, first = run(two_levels, [DOWN, ENTER, DOWN, "q"])
    _vi, second = run(two_levels, ["j", "l", "j", "q"])
    assert frames(first) == frames(second)


def test_the_cursor_wraps_at_both_ends():
    _picks, sheet = run(two_levels, [UP, "q"])
    assert rows(frames(sheet)[-1])[-1] == "  ❯ dry"


def test_digits_jump_within_the_active_column():
    _picks, sheet = run(two_levels, ["3", "q"])
    assert rows(frames(sheet)[-1])[2] == "  ❯ status"


def test_a_superscript_digit_is_not_a_jump():
    _picks, sheet = run(two_levels, ["²", "q"])
    assert frames(sheet)[0] == frames(sheet)[-1]


def test_a_child_taller_than_the_room_below_its_parent_is_pulled_up(monkeypatch):
    monkeypatch.setattr(menu.shutil, "get_terminal_size", screen(20))
    parent = Column([f"row {number}" for number in range(8)], kind="menu", default=7)
    child = Column([f"leaf {number}" for number in range(14)], kind="leaf")
    _picks, sheet = run(lambda picks: child if picks else parent, [ENTER, "q"])
    body = rows(frames(sheet)[-1])
    assert len(body) == 15
    assert body[0] == "    row 0"
    assert body[1] == "    row 1    ❯ leaf 0"
    assert body[7] == "  ❯ row 7      leaf 6"
    assert body[14] == "               leaf 13"


def test_a_narrow_terminal_drops_the_leftmost_column(monkeypatch):
    monkeypatch.setattr(menu.shutil, "get_terminal_size", lambda: os.terminal_size((34, 40)))
    _picks, sheet = run(two_levels, [DOWN, ENTER, "q"])
    frame = frames(sheet)[-1]
    assert frame[1] == "  dotfile theme  ‹ switch"
    assert rows(frame)[0] == "  ❯ global        mocha"
    assert rows(frame)[2] == "    linux/common  mocha   gtk, q…"


def test_a_column_title_sits_above_its_options():
    def expand(picks):
        if not picks:
            return Column(MENU, kind="menu")
        return Column(["alpha", "beta"], kind="side", title="which side?")

    _picks, sheet = run(expand, [ENTER, "q"])
    assert rows(frames(sheet)[-1]) == [
        "  ❯ sync       which side?",
        "    switch     ❯ alpha",
        "    status       beta",
        "    preview",
        "    dry",
    ]


def test_the_panel_renders_below_the_block():
    column = Column(MENU, kind="menu", preview=lambda index: [f"card {index}"] * 3)
    _picks, sheet = run(lambda picks: None if picks else column, [DOWN, "q"])
    frame = frames(sheet)[-1]
    assert frame[-4:] == ["", "card 1", "card 1", "card 1"]


def test_the_panel_is_dropped_when_the_screen_is_short(monkeypatch):
    monkeypatch.setattr(menu.shutil, "get_terminal_size", screen(12))
    column = Column(MENU, kind="menu", preview=lambda index: ["card"] * 6)
    _picks, sheet = run(lambda picks: None if picks else column, ["q"])
    assert frames(sheet)[-1][-1] == "    dry"


def test_panel_lines_are_not_clipped_by_the_composer():
    painted = "\x1b[48;2;30;30;46m" + " " * 40 + "\x1b[0m"
    column = Column(MENU, kind="menu", preview=lambda index: [painted])
    sheet = Screen()
    menu.cascade("t", lambda picks: None if picks else column, keys=["q"], out=sheet)
    assert painted in sheet.getvalue()


def test_no_preview_means_no_panels():
    assert menu._panels(None, 3, 30) == []


def test_panels_are_padded_to_a_single_height():
    panels = menu._panels(lambda index: ["line"] * (index + 1), 3, 31)
    assert [len(panel) for panel in panels] == [3, 3, 3]
    assert panels[0] == ["line", "", ""]


def test_panels_are_clipped_to_the_room_that_is_left():
    panels = menu._panels(lambda index: ["line"] * 10, 3, 5)
    assert [len(panel) for panel in panels] == [5, 5, 5]


def test_an_empty_preview_is_treated_as_none():
    assert menu._panels(lambda index: [], 2, 30) == []
