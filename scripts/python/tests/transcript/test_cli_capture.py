import pytest
import typer

from tools.core import clipboard
from tools.transcript import cli


def test_capture_uses_fallback_when_clipboard_empty(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    monkeypatch.setattr(clipboard, "read_text", lambda: "")
    snapshot = tmp_path / "snap"
    snapshot.write_text("\x1b[1mhello from the copy button\x1b[0m")
    cli.capture(provider="agent", raw=False, quiet=True, fallback=str(snapshot))
    notes = list((tmp_path / "vault" / "Transcripts").rglob("*.md"))
    assert len(notes) == 1
    text = notes[0].read_text()
    assert "hello from the copy button" in text
    assert "\x1b" not in text
    assert not snapshot.exists()


def test_capture_prefers_clipboard_over_fallback(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    monkeypatch.setattr(clipboard, "read_text", lambda: "fresh selection")
    snapshot = tmp_path / "snap"
    snapshot.write_text("stale snapshot")
    cli.capture(provider="agent", raw=False, quiet=True, fallback=str(snapshot))
    notes = list((tmp_path / "vault" / "Transcripts").rglob("*.md"))
    assert "fresh selection" in notes[0].read_text()
    assert snapshot.exists()


def test_capture_dies_when_everything_empty(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    monkeypatch.setattr(clipboard, "read_text", lambda: "")
    with pytest.raises(typer.Exit):
        cli.capture(provider="", raw=False, quiet=True, fallback="")
