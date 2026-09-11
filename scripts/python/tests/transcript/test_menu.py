import io
import os
import re

import pytest

from tools.transcript import menu
from tools.transcript.menu import Column

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
JUMP = re.compile(r"\x1b\[(\d+)A")

UP = "\x1b[A"
DOWN = "\x1b[B"
RIGHT = "\x1b[C"
LEFT = "\x1b[D"
ENTER = "\r"

MENU = ["sync", "import", "list", "capture", "add"]
PROJECTS = ["webapp", "server/api", "infra/common"]
DETAILS = ["Codex", "Claude   sessions", "Codex   archived session"]


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
    if picks[-1].kind == "menu" and picks[-1].option == "import":
        return Column(PROJECTS, DETAILS, kind="project")
    return None


def run(expand, keys, title="transcript", start=()):
    sheet = Screen()
    picks = menu.cascade(title, expand, start=start, keys=keys, out=sheet)
    return picks, sheet


def rows(frame):
    return frame[4:]


def test_cascade_needs_a_terminal(monkeypatch):
    monkeypatch.setattr(menu.sys, "stdout", Screen(False))
    assert menu.cascade("transcript", two_levels, keys=[ENTER]) is None


@pytest.mark.parametrize("root", [None, Column([])])
def test_an_empty_cascade_returns_without_drawing_and_restores_the_cursor(root):
    picks, sheet = run(lambda _picks: root, [ENTER])
    assert picks is None
    assert sheet.getvalue() == menu.HIDE + menu.SHOW


def test_a_single_column_frame_reads_like_a_plain_list():
    _picks, sheet = run(lambda picks: None if picks else Column(MENU, kind="menu"), ["q"])
    assert frames(sheet)[0] == [
        "",
        "  transcript",
        "  ↑/↓ move | ↩ select | q quit",
        "",
        "  ❯ sync",
        "    import",
        "    list",
        "    capture",
        "    add",
    ]


def test_a_child_column_opens_beside_its_parent():
    _picks, sheet = run(two_levels, [DOWN, ENTER, DOWN, "q"])
    assert rows(frames(sheet)[-1]) == [
        "    sync",
        "  ❯ import       webapp        Codex",
        "    list       ❯ server/api    Claude   sessions",
        "    capture      infra/common  Codex   archived session",
        "    add",
    ]


def test_only_the_active_column_shows_details():
    def expand(picks):
        column = two_levels(picks)
        if column is None and picks[-1].kind == "project":
            return Column(["first", "latest"], ["full note", "summary"], kind="session")
        return column

    _picks, sheet = run(expand, [DOWN, ENTER, DOWN, DOWN, ENTER, "q"])
    body = rows(frames(sheet)[-1])
    assert "Codex   archived session" not in "\n".join(body)
    assert body == [
        "    sync",
        "  ❯ import       webapp",
        "    list         server/api",
        "    capture    ❯ infra/common    ❯ first   full note",
        "    add                            latest  summary",
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
        menu.Pick("menu", 1, "import"),
        menu.Pick("project", 1, "server/api"),
    ]


def test_reopening_a_cascade_restores_the_selected_path():
    picks, _sheet = run(two_levels, [ENTER], start=(1, 1))
    assert picks == [
        menu.Pick("menu", 1, "import"),
        menu.Pick("project", 1, "server/api"),
    ]


def test_quitting_deep_returns_nothing():
    picks, _sheet = run(two_levels, [DOWN, ENTER, DOWN, "q"])
    assert picks is None


def test_a_finished_cascade_collapses_to_one_line():
    _picks, sheet = run(two_levels, [DOWN, ENTER, DOWN, ENTER])
    assert tail(sheet).splitlines() == ["  transcript — import › server/api"]


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
    assert rows(frames(sheet)[-1])[-1] == "  ❯ add"


def test_digits_jump_within_the_active_column():
    _picks, sheet = run(two_levels, ["3", "q"])
    assert rows(frames(sheet)[-1])[2] == "  ❯ list"


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
    assert frame[1] == "  transcript  ‹ import"
    assert rows(frame)[0] == "  ❯ webapp        Codex"
    assert rows(frame)[2] == "    infra/common  Codex   archiv…"
