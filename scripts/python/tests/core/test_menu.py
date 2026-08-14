import os

from tools.core import menu


class Stream:
    def __init__(self, tty):
        self.tty = tty

    def isatty(self):
        return self.tty


def screen(lines):
    return lambda: os.terminal_size((80, lines))


def test_pick_needs_a_terminal(monkeypatch):
    monkeypatch.setattr(menu.sys, "stdout", Stream(False))
    assert menu.pick("pick one", ["a", "b"]) is None


def test_pick_needs_options(monkeypatch):
    monkeypatch.setattr(menu.sys, "stdout", Stream(True))
    assert menu.pick("pick one", []) is None


def test_no_preview_means_no_panels():
    assert menu._panels(None, 3) == []


def test_panels_are_padded_to_a_single_height(monkeypatch):
    monkeypatch.setattr(menu.shutil, "get_terminal_size", screen(40))
    panels = menu._panels(lambda index: ["line"] * (index + 1), 3)
    assert [len(panel) for panel in panels] == [3, 3, 3]
    assert panels[0] == ["line", "", ""]


def test_panels_are_dropped_when_the_screen_is_short(monkeypatch):
    monkeypatch.setattr(menu.shutil, "get_terminal_size", screen(9))
    assert menu._panels(lambda index: ["line"] * 10, 3) == []


def test_panels_are_clipped_to_the_room_that_is_left(monkeypatch):
    monkeypatch.setattr(menu.shutil, "get_terminal_size", screen(14))
    panels = menu._panels(lambda index: ["line"] * 10, 3)
    assert [len(panel) for panel in panels] == [5, 5, 5]


def test_an_empty_preview_is_treated_as_none():
    assert menu._panels(lambda index: [], 2) == []
