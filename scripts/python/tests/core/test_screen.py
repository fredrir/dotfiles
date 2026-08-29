import contextlib
import os
import time
import tty

import pytest

from tools.core import screen


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


def test_a_bare_escape_returns_without_waiting_for_more():
    with terminal_pair() as (reader, writer):
        os.write(writer, b"\x1b")
        started = time.monotonic()
        assert screen.read_key(reader) == "\x1b"
        assert time.monotonic() - started < 1


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
