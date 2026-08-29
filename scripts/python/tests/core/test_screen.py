import contextlib
import io
import os
import time
import tty

import pytest

from tools.core import menu, screen
from tools.core.menu import Column


@contextlib.contextmanager
def terminal_pair():
    """A raw pty, so the reader can be exercised the way it runs against /dev/tty."""
    reader, writer = os.openpty()
    tty.setraw(writer)
    try:
        yield reader, writer
    finally:
        for handle in (reader, writer):
            with contextlib.suppress(OSError):
                os.close(handle)


@pytest.mark.parametrize(
    ("key", "name"),
    [
        ("\x1b[A", "up"),
        ("\x1bOA", "up"),
        ("\x1b[B", "down"),
        ("\x1bOB", "down"),
        ("\x1b[C", "right"),
        ("\x1bOC", "right"),
        ("\x1b[D", "left"),
        ("\x1bOD", "left"),
        ("k", "up"),
        ("j", "down"),
        ("h", "left"),
        ("l", "right"),
        ("\r", "enter"),
        ("\n", "enter"),
        ("q", "q"),
    ],
)
def test_keys_are_named_consistently(key, name):
    assert screen.normalise(key) == name


def test_an_arrow_sequence_is_read_as_one_keystroke():
    with terminal_pair() as (reader, writer):
        os.write(writer, b"\x1b[A")
        assert screen.read_key(reader) == "\x1b[A"


@pytest.mark.parametrize(
    "sequence", ["\x1b[A", "\x1bOC", "\x1b[1;2D", "\x1b[15;5~", "\x1b[?2026;2$y"]
)
def test_an_escape_sequence_is_consumed_whole(sequence):
    with terminal_pair() as (reader, writer):
        os.write(writer, sequence.encode() + b"j")
        assert screen.read_key(reader) == sequence
        assert screen.read_key(reader) == "j"


def test_a_bare_escape_returns_without_waiting_for_more():
    with terminal_pair() as (reader, writer):
        os.write(writer, b"\x1b")
        started = time.monotonic()
        assert screen.read_key(reader) == "\x1b"
        assert time.monotonic() - started < screen.ESC_DELAY * 4


@pytest.mark.parametrize(
    "sequence",
    [
        "\x1b[?65;1;2;6;9;15;16;17;18;21;22;28;32c",
        "\x1b[<1024;9999;9999M",
        "\x1b[97:97:97;5:3;97u",
    ],
)
def test_a_sequence_past_the_cap_is_drained_not_left_behind(sequence):
    with terminal_pair() as (reader, writer):
        os.write(writer, sequence.encode() + b"\r")
        assert screen.normalise(screen.read_key(reader)) not in screen.ABORT
        assert screen.normalise(screen.read_key(reader)) == "enter"


@pytest.mark.parametrize("payload", ["ø".encode(), b"\x1b" + "å".encode()])
def test_an_undecodable_byte_is_ignored_rather_than_read_as_an_abort(payload):
    with terminal_pair() as (reader, writer):
        os.write(writer, payload + b"\r")
        while (key := screen.normalise(screen.read_key(reader))) != "enter":
            assert key not in screen.ABORT


def test_a_truncated_sequence_gives_up_instead_of_blocking():
    with terminal_pair() as (reader, writer):
        os.write(writer, b"\x1b[")
        started = time.monotonic()
        assert screen.read_key(reader) == "\x1b["
        assert time.monotonic() - started < screen.ESC_DELAY * 4


def test_a_closed_terminal_reads_as_an_abort():
    with terminal_pair() as (reader, writer):
        os.close(writer)
        assert screen.read_key(reader) == ""


def test_fit_marks_the_cut_with_an_ellipsis():
    assert screen.fit("linux/common", 20) == "linux/common"
    assert screen.fit("linux/common", 8) == "linux/c…"
    assert screen.fit("linux/common", 1) == "…"
    assert screen.fit("linux/common", 0) == ""


def test_compose_reports_the_printed_width_not_the_escaped_length():
    line, used = screen.compose([("abc", screen.BOLD), ("de", "")], 40, True)
    assert used == 5
    assert screen.visible(line) == 5
    assert line.startswith(screen.BOLD)


def test_compose_clips_cumulatively_and_drops_what_will_not_fit():
    line, used = screen.compose([("abcdef", ""), ("ghi", "")], 4, False)
    assert (line, used) == ("abc…", 4)


def one_column(picks):
    return None if picks else Column(["alpha", "bravo", "charlie"], kind="menu")


@pytest.mark.parametrize("sequence", [b"\x1b[1;2D", b"\x1b[?65;1;6;9;15;18;21;28;2c"])
def test_a_sequence_the_picker_cannot_name_leaves_the_selection_alone(sequence):
    with terminal_pair() as (reader, writer):
        os.write(writer, sequence + b"\r")
        picks = menu.cascade(
            "keys", one_column, keys=lambda: screen.read_key(reader), out=io.StringIO()
        )
    assert [pick.option for pick in picks] == ["alpha"]
