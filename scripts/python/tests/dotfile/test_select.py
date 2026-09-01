import io
import re

import pytest

from tools.dotfile import select
from tools.dotfile.select import Change, resolve

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
JUMP = re.compile(r"\x1b\[(\d+)A")

UP = "\x1b[A"
DOWN = "\x1b[B"
RIGHT = "\x1b[C"
LEFT = "\x1b[D"
ENTER = "\r"
ARROW = "  → "
ESC = "\x1b"
INTERRUPT = "\x03"

DST = "~/Library/Application Support/Code/User/settings.json"


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


@pytest.fixture
def changes():
    return [
        Change("add", ("editor.fontSize",), '"editor.fontSize"', "14", ["macos", "linux"]),
        Change("modify", ("git.autofetch",), '"git.autofetch"', "true → false", ["macos", "linux"]),
        Change("delete", ("cSpell.userWords",), '"cSpell.userWords"', '["fredrir", …]', ["macos"]),
    ]


def plain(text):
    return ANSI.sub("", text)


def frames(screen):
    """Every painted frame, as lists of un-coloured lines."""
    chunks = JUMP.split(screen.getvalue())[::2]
    return [plain(chunk).split("\n")[:-1] for chunk in chunks if chunk]


def rows(frame):
    """The change rows: everything between the two scroll hints."""
    return frame[6:-3]


def staged(frame):
    """The pending choice shown on each row, "" where the row is still undecided."""
    return [line.split(ARROW)[1] if ARROW in line else "" for line in rows(frame)]


def run(changes, keys, terminal=False):
    screen = Screen(terminal)
    return resolve(DST, changes, keys=keys, out=screen), screen


def test_first_frame_matches_the_intended_layout(changes):
    _plan, screen = run(changes, ["q"])
    assert frames(screen)[0] == [
        "",
        "  dotfile sync | 3 changes",
        "  ↑/↓ key | ←/→ action | ⏎ apply | a rest | u undo | q abort",
        "",
        f"  {DST}",
        "",
        '  ❯ + "editor.fontSize"   14',
        '    ~ "git.autofetch"     true → false',
        '    - "cSpell.userWords"  ["fredrir", …]',
        "",
        "    ❯ shared    target    ignore    discard",
        "",
    ]


def test_forward_walk_applies_one_action_per_row(changes):
    keys = [ENTER, RIGHT, RIGHT, ENTER, RIGHT, ENTER, ENTER]
    plan, _screen = run(changes, keys)
    assert plan == {0: "shared", 1: "ignore", 2: "discard"}


def test_enter_advances_to_the_next_undecided_row(changes):
    _plan, screen = run(changes, [ENTER, "q"])
    assert '  ❯ ~ "git.autofetch"' in "\n".join(rows(frames(screen)[1]))


def test_decided_rows_show_their_staged_choice(changes):
    _plan, screen = run(changes, [ENTER, "q"])
    assert staged(frames(screen)[1]) == ["shared", "", ""]


def test_advance_skips_rows_that_are_already_decided(changes):
    # decide row 1, come back to row 0, decide it: the cursor must land on row 2.
    _plan, screen = run(changes, [DOWN, ENTER, UP, UP, ENTER, "q"])
    assert '  ❯ - "cSpell.userWords"' in "\n".join(rows(frames(screen)[-2]))


def test_row_navigation_wraps(changes):
    _plan, screen = run(changes, [UP, "q"])
    assert '  ❯ - "cSpell.userWords"' in "\n".join(rows(frames(screen)[1]))


def test_vi_keys_match_the_arrow_keys(changes):
    walk = [DOWN, DOWN, RIGHT, RIGHT, RIGHT, ENTER, UP, UP, LEFT, LEFT, LEFT, ENTER, ENTER, ENTER]
    vi = ["j", "j", "l", "l", "l", ENTER, "k", "k", "h", "h", "h", ENTER, ENTER, ENTER]
    arrows, first = run(changes, walk)
    letters, second = run(changes, vi)
    assert arrows == letters == {0: "shared", 1: "shared", 2: "discard"}
    assert frames(first) == frames(second)


def test_digits_jump_straight_to_an_action(changes):
    plan, _screen = run(changes, ["4", ENTER, "1", ENTER, "3", ENTER, ENTER])
    assert plan == {0: "discard", 1: "shared", 2: "ignore"}


def test_target_opens_a_submenu_of_overlay_names(changes):
    _plan, screen = run(changes, [RIGHT, ENTER, "q"])
    frame = frames(screen)[-2]
    assert frame[-1].strip().startswith("‹ back")
    assert "macos" in frame[-1] and "linux" in frame[-1]
    assert frame[2] == "  ←/→ target | ⏎ choose | a rest | ‹ back | q abort"


def test_submenu_aligns_under_the_selected_action(changes):
    _plan, screen = run(changes, [RIGHT, ENTER, "q"])
    frame = frames(screen)[-2]
    assert frame[-1].index("‹ back") - 2 == frame[-2].index("❯ target")


def test_submenu_picks_an_overlay(changes):
    plan, _screen = run(changes, [RIGHT, ENTER, RIGHT, ENTER, LEFT, ENTER, ENTER, ENTER])
    assert plan == {0: "target:linux", 1: "shared", 2: "shared"}


def test_submenu_back_returns_to_the_action_bar(changes):
    _plan, screen = run(changes, [RIGHT, ENTER, LEFT, ENTER, "q"])
    frame = frames(screen)[-2]
    assert frame[-1] == ""
    assert frame[2].startswith("  ↑/↓ key")


def test_submenu_back_stages_nothing(changes):
    plan, _screen = run(changes, [RIGHT, ENTER, LEFT, ENTER, LEFT, ENTER, ENTER, ENTER, ENTER])
    assert plan == {0: "shared", 1: "shared", 2: "shared"}


def test_a_row_without_overlays_cannot_reach_the_submenu():
    changes = [Change("add", ("solo",), '"solo"', "1"), Change("add", ("two",), '"two"', "2")]
    _plan, screen = run(changes, [RIGHT, ENTER, "q"])
    frame = frames(screen)[-2]
    assert frame[-1] == ""
    assert staged(frame) == ["", ""]


def test_bulk_applies_the_action_to_every_undecided_row(changes):
    plan, _screen = run(changes, ["a", ENTER])
    assert plan == {0: "shared", 1: "shared", 2: "shared"}


def test_bulk_leaves_already_decided_rows_alone(changes):
    plan, _screen = run(changes, [ENTER, RIGHT, RIGHT, "a", ENTER])
    assert plan == {0: "shared", 1: "ignore", 2: "ignore"}


def test_bulk_target_skips_rows_that_lack_that_overlay(changes):
    # only rows 0 and 1 offer "linux"; row 2 stays undecided and is discarded by hand.
    plan, _screen = run(changes, [RIGHT, "a", RIGHT, ENTER, "4", ENTER, ENTER])
    assert plan == {0: "target:linux", 1: "target:linux", 2: "discard"}


def test_bulk_from_inside_the_submenu(changes):
    plan, _screen = run(changes, [RIGHT, ENTER, "a", "1", ENTER, ENTER])
    assert plan == {0: "target:macos", 1: "target:macos", 2: "target:macos"}


def test_undo_reverts_the_most_recent_decision(changes):
    _plan, screen = run(changes, [ENTER, ENTER, "u", "q"])
    frame = frames(screen)[-2]
    assert frame[1] == "  dotfile sync | 3 changes | 1 decided"
    assert '  ❯ ~ "git.autofetch"' in "\n".join(rows(frame))


def test_undo_restores_an_earlier_choice_rather_than_clearing_it(changes):
    plan, _screen = run(changes, [ENTER, ENTER, ENTER, RIGHT, ENTER, "4", ENTER, "u", ENTER])
    assert plan == {0: "shared", 1: "shared", 2: "shared"}


def test_undo_reverts_a_bulk_apply_as_one_step(changes):
    _plan, screen = run(changes, ["a", "u", "q"])
    frame = frames(screen)[-2]
    assert frame[1] == "  dotfile sync | 3 changes"
    assert staged(frame) == ["", "", ""]


def test_undo_with_nothing_staged_is_harmless(changes):
    plan, _screen = run(changes, ["u", "u", "a", ENTER])
    assert plan == {0: "shared", 1: "shared", 2: "shared"}


def test_nothing_is_returned_until_the_plan_is_confirmed(changes):
    plan, screen = run(changes, ["a", "q"])
    assert plan is None
    assert frames(screen)[-2][-2] == "    ❯ apply 3 changes   ‹ revise"


def test_confirm_frame_shows_the_whole_plan(changes):
    _plan, screen = run(changes, [ENTER, RIGHT, RIGHT, ENTER, RIGHT, ENTER, "q"])
    frame = frames(screen)[-2]
    assert staged(frame) == ["shared", "ignore", "discard"]
    assert frame[2] == "  ↑/↓ review | ←/→ choose | ⏎ confirm | u undo | q abort"


def test_revise_returns_to_the_action_bar(changes):
    walk = [ENTER, ENTER, ENTER, RIGHT, ENTER, UP, UP, "4", ENTER, ENTER]
    plan, _screen = run(changes, walk)
    assert plan == {0: "discard", 1: "shared", 2: "shared"}


def test_undo_at_the_confirm_prompt_reopens_editing(changes):
    _plan, screen = run(changes, ["a", "u", "q"])
    assert frames(screen)[-2][2].startswith("  ↑/↓ key")


def test_confirming_closes_with_a_summary(changes):
    plan, screen = run(changes, ["a", ENTER])
    assert plan == {0: "shared", 1: "shared", 2: "shared"}
    assert frames(screen)[-1][-2] == "    ✓ applying 3 changes"


@pytest.mark.parametrize("key", ["q", ESC, INTERRUPT, ""])
def test_every_abort_key_returns_nothing(changes, key):
    plan, screen = run(changes, [ENTER, key])
    assert plan is None
    assert frames(screen)[-1][-2] == "    ✗ aborted | nothing applied"


def test_abort_at_the_confirm_prompt_discards_the_whole_plan(changes):
    plan, screen = run(changes, [ENTER, ENTER, ENTER, "q"])
    assert plan is None
    assert frames(screen)[-1][-2] == "    ✗ aborted | nothing applied"


def test_running_out_of_keys_aborts_rather_than_hanging(changes):
    plan, _screen = run(changes, [ENTER])
    assert plan is None


def test_a_keyboard_interrupt_aborts_cleanly(changes):
    def interrupt():
        raise KeyboardInterrupt

    plan, screen = run(changes, interrupt)
    assert plan is None
    assert screen.getvalue().endswith(select.SHOW)


def test_the_cursor_is_hidden_and_always_restored(changes):
    _plan, screen = run(changes, [ENTER, "q"])
    text = screen.getvalue()
    assert text.startswith(select.HIDE)
    assert text.endswith(select.SHOW)
    assert text.count(select.HIDE) == text.count(select.SHOW) == 1


def test_the_cursor_is_restored_when_the_reader_explodes(changes):
    def broken():
        raise RuntimeError("tty went away")

    screen = Screen()
    with pytest.raises(RuntimeError):
        resolve(DST, changes, keys=broken, out=screen)
    assert screen.getvalue().endswith(select.SHOW)


def test_no_terminal_returns_none_without_painting(changes, monkeypatch):
    def missing():
        raise OSError("no /dev/tty")

    monkeypatch.setattr(select, "tty_keys", missing)
    screen = Screen()
    assert resolve(DST, changes, out=screen) is None
    assert screen.getvalue() == ""


def test_an_empty_change_list_needs_no_interaction():
    screen = Screen()
    assert resolve(DST, [], out=screen) == {}
    assert screen.getvalue() == ""


def test_colour_is_used_on_a_terminal(changes):
    _plan, screen = run(changes, ["q"], terminal=True)
    assert select.CYAN in screen.getvalue()


def test_no_colour_when_the_stream_is_not_a_terminal(changes):
    _plan, screen = run(changes, ["q"])
    assert select.CYAN not in screen.getvalue()
    assert select.HIDE in screen.getvalue()


def test_no_colour_when_no_color_is_set(changes, monkeypatch):
    monkeypatch.setenv("NO_COLOR", "1")
    _plan, screen = run(changes, ["q"], terminal=True)
    assert select.CYAN not in screen.getvalue()


def test_long_values_are_truncated_to_the_terminal_width(monkeypatch):
    monkeypatch.setenv("COLUMNS", "56")
    changes = [
        Change("add", ("a",), '"workbench.colorCustomizations"', "{" + "x" * 200 + "}", ["macos"]),
        Change("conflict", ("b",), '"editor.fontFamily"', '"JetBrains Mono" → "Berkeley"'),
    ]
    _plan, screen = run(changes, ["q"])
    frame = frames(screen)[0]
    assert max(len(line) for line in frame) <= 55
    assert all(line.endswith("…") for line in rows(frame))


def test_a_list_taller_than_the_terminal_scrolls_with_the_cursor(monkeypatch):
    monkeypatch.setenv("LINES", "16")
    changes = [Change("modify", (f"k{i}",), f'"key.{i}"', f"{i}") for i in range(12)]
    _plan, screen = run(changes, [DOWN] * 8 + ["q"])
    painted = frames(screen)
    assert {len(frame) for frame in painted} == {15}
    assert len(rows(painted[0])) == 6
    assert painted[0][5] == ""
    assert painted[0][-3] == "    ⋯ 6 more below"
    last = painted[-2]
    assert last[5] == "    ⋯ 3 more above"
    assert last[-3] == "    ⋯ 3 more below"
    assert '  ❯ ~ "key.8"' in "\n".join(rows(last))


def test_redraw_repaints_in_place_at_a_constant_height(changes):
    _plan, screen = run(changes, [DOWN, RIGHT, ENTER, "q"])
    text = screen.getvalue()
    assert set(JUMP.findall(text)) == {"12"}
    assert text.count("\x1b[J") == 0


def test_row_glyphs_follow_the_change_kind():
    changes = [
        Change("add", ("a",), '"a"', "1"),
        Change("modify", ("b",), '"b"', "2"),
        Change("delete", ("c",), '"c"', "3"),
        Change("conflict", ("d",), '"d"', "4"),
        Change("mystery", ("e",), '"e"', "5"),
    ]
    _plan, screen = run(changes, ["q"])
    assert [line[4] for line in rows(frames(screen)[0])] == ["+", "~", "-", "!", "|"]


def test_the_change_record_keeps_its_key_path():
    change = Change("modify", ["[lua]", "editor.tabSize"], '"editor.tabSize"', "4 → 2")
    assert change.path == ("[lua]", "editor.tabSize")
    assert change.targets == []
    assert "modify" in repr(change)


def test_application_cursor_keys_drive_the_selector(changes):
    plan, _screen = run(changes, ["\x1bOB", "\x1bOC", "\x1bOC", ENTER, ENTER, ENTER, ENTER])
    assert plan == {0: "ignore", 1: "ignore", 2: "ignore"}
