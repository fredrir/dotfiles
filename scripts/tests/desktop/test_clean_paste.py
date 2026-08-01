from tools.desktop import clean_paste


def test_strips_ansi_escapes():
    assert clean_paste.clean_text("\x1b[1;32mhello\x1b[0m world") == "hello world"


def test_strips_osc_sequences():
    assert clean_paste.clean_text("\x1b]0;title\x07echo hi") == "echo hi"


def test_normalises_crlf():
    assert clean_paste.clean_text("a\r\nb\rc") == "a\nb\nc"


def test_replaces_nonbreaking_and_zero_width_spaces():
    assert clean_paste.clean_text("a\u00a0b\u200bc") == "a bc"


def test_drops_stray_control_characters_but_keeps_tabs():
    assert clean_paste.clean_text("a\x08b\tc") == "ab\tc"


def test_removes_trailing_whitespace_per_line():
    assert clean_paste.clean_text("one   \ntwo\t\n") == "one\ntwo"


def test_dedents_common_space_indentation():
    text = "        def f():\n            return 1\n"
    assert clean_paste.clean_text(text) == "def f():\n    return 1"


def test_dedents_common_tab_indentation():
    assert clean_paste.clean_text("\t\tfoo\n\t\t\tbar") == "foo\n\tbar"


def test_mixed_indentation_only_strips_the_common_prefix():
    assert clean_paste.clean_text("    a\n\tb") == "    a\n\tb"


def test_blank_lines_do_not_break_dedent():
    text = "    first\n\n    second"
    assert clean_paste.clean_text(text) == "first\n\nsecond"


def test_trims_leading_and_trailing_blank_lines():
    assert clean_paste.clean_text("\n\n  x\n\n\n") == "x"


def test_single_line_is_fully_dedented():
    assert clean_paste.clean_text("      uv sync --locked") == "uv sync --locked"


def test_rewrites_the_clipboard_when_text_is_available(monkeypatch):
    written = []
    monkeypatch.setattr(clean_paste, "read_clipboard", lambda: "  hello  ")
    monkeypatch.setattr(clean_paste, "write_clipboard", written.append)
    clean_paste.clean_paste()
    assert written == ["hello"]


def test_leaves_non_text_clipboard_alone(monkeypatch):
    written = []
    monkeypatch.setattr(clean_paste, "read_clipboard", lambda: None)
    monkeypatch.setattr(clean_paste, "write_clipboard", written.append)
    clean_paste.clean_paste()
    assert written == []


def test_leaves_whitespace_only_clipboard_alone(monkeypatch):
    written = []
    monkeypatch.setattr(clean_paste, "read_clipboard", lambda: "   \n  \n")
    monkeypatch.setattr(clean_paste, "write_clipboard", written.append)
    clean_paste.clean_paste()
    assert written == []
