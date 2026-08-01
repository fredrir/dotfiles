from io import StringIO

from tools.desktop import clean_copy


def test_strips_ansi_escapes():
    assert clean_copy.clean_text("\x1b[1;32mhello\x1b[0m world") == "hello world"


def test_strips_osc_sequences():
    assert clean_copy.clean_text("\x1b]0;title\x07echo hi") == "echo hi"


def test_normalises_crlf():
    assert clean_copy.clean_text("a\r\nb\rc") == "a\nb\nc"


def test_replaces_nonbreaking_and_zero_width_spaces():
    assert clean_copy.clean_text("a\u00a0b\u200bc") == "a bc"


def test_drops_stray_control_characters_but_keeps_tabs():
    assert clean_copy.clean_text("a\x08b\tc") == "ab\tc"


def test_removes_trailing_whitespace_per_line():
    assert clean_copy.clean_text("one   \ntwo\t\n") == "one\ntwo"


def test_dedents_common_space_indentation():
    text = "        def f():\n            return 1\n"
    assert clean_copy.clean_text(text) == "def f():\n    return 1"


def test_dedents_common_tab_indentation():
    assert clean_copy.clean_text("\t\tfoo\n\t\t\tbar") == "foo\n\tbar"


def test_mixed_indentation_only_strips_the_common_prefix():
    assert clean_copy.clean_text("    a\n\tb") == "    a\n\tb"


def test_blank_lines_do_not_break_dedent():
    text = "    first\n\n    second"
    assert clean_copy.clean_text(text) == "first\n\nsecond"


def test_trims_leading_and_trailing_blank_lines():
    assert clean_copy.clean_text("\n\n  x\n\n\n") == "x"


def test_single_line_is_fully_dedented():
    assert clean_copy.clean_text("      uv sync --locked") == "uv sync --locked"


def test_rewrites_the_clipboard_when_text_is_available(monkeypatch):
    written = []
    monkeypatch.setattr(clean_copy, "read_clipboard", lambda: "  hello  ")
    monkeypatch.setattr(clean_copy, "write_clipboard", written.append)
    clean_copy.clean_copy()
    assert written == ["hello"]


def test_rewrites_selected_text_from_stdin(monkeypatch):
    written = []
    monkeypatch.setattr(clean_copy.sys, "stdin", StringIO("    hello\n"))
    monkeypatch.setattr(clean_copy, "write_clipboard", written.append)
    clean_copy.clean_copy(stdin=True)
    assert written == ["hello"]


def test_leaves_non_text_clipboard_alone(monkeypatch):
    written = []
    monkeypatch.setattr(clean_copy, "read_clipboard", lambda: None)
    monkeypatch.setattr(clean_copy, "write_clipboard", written.append)
    clean_copy.clean_copy()
    assert written == []


def test_leaves_whitespace_only_clipboard_alone(monkeypatch):
    written = []
    monkeypatch.setattr(clean_copy, "read_clipboard", lambda: "   \n  \n")
    monkeypatch.setattr(clean_copy, "write_clipboard", written.append)
    clean_copy.clean_copy()
    assert written == []
