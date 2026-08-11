from types import SimpleNamespace

import pytest
import typer
from typer.testing import CliRunner

from tools.core import clipboard
from tools.utils import remote_clipboard


def completed(returncode=0, stdout=b"", stderr=b""):
    return SimpleNamespace(returncode=returncode, stdout=stdout, stderr=stderr)


def test_sends_exact_utf8_text_over_ssh_stdin(monkeypatch, capsys):
    calls = []
    text = "hei ☕\nnext\n"
    monkeypatch.setattr(clipboard, "read_text", lambda: text)

    def fake_run(command, **kwargs):
        calls.append((command, kwargs))
        return completed()

    monkeypatch.setattr(remote_clipboard, "run", fake_run)
    remote_clipboard.send_to_archie()

    command, kwargs = calls[0]
    assert command[:5] == ["ssh", "-T", "-o", "ConnectTimeout=5", "archie"]
    assert "systemctl --user show-environment" in command[-1]
    assert "exec wl-copy --type text/plain" in command[-1]
    assert "--sensitive" not in command[-1]
    assert kwargs["input"] == text.encode("utf-8")
    assert capsys.readouterr().out == "clipboard → archie\n"


def test_sensitive_copy_uses_fixed_wl_copy_option(monkeypatch, capsys):
    calls = []
    monkeypatch.setattr(clipboard, "read_text", lambda: "secret")
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda command, **kwargs: calls.append((command, kwargs)) or completed(),
    )

    remote_clipboard.send_to_archie(sensitive=True, prog="cpas")

    assert calls[0][0][-1].endswith("exec wl-copy --type text/plain --sensitive")
    assert capsys.readouterr().out == "clipboard → archie (sensitive)\n"


@pytest.mark.parametrize("text", [None, "", " \n\t"])
def test_rejects_unusable_local_clipboard_without_starting_ssh(monkeypatch, text):
    monkeypatch.setattr(clipboard, "read_text", lambda: text)
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda *_args, **_kwargs: pytest.fail("ssh should not be started"),
    )

    with pytest.raises(typer.Exit):
        remote_clipboard.send_to_archie()


def test_reports_remote_copy_failure(monkeypatch, capsys):
    monkeypatch.setattr(clipboard, "read_text", lambda: "hello")
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda *_args, **_kwargs: completed(
            returncode=20,
            stderr=b"remote clipboard: no active Wayland session\n",
        ),
    )

    with pytest.raises(typer.Exit):
        remote_clipboard.send_to_archie()

    assert "no active Wayland session" in capsys.readouterr().err


def test_receives_exact_utf8_text_before_writing_local_clipboard(monkeypatch, capsys):
    written = []
    text = "fra archie → mac\n"
    calls = []
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda command, **kwargs: (
            calls.append((command, kwargs)) or completed(stdout=text.encode("utf-8"))
        ),
    )
    monkeypatch.setattr(clipboard, "write_text", lambda value: written.append(value) or True)

    remote_clipboard.receive_from_archie()

    assert "exec wl-paste --no-newline --type text" in calls[0][0][-1]
    assert written == [text]
    assert capsys.readouterr().out == "archie → clipboard\n"


@pytest.mark.parametrize("stdout", [b"", b" \n\t", b"\xff"])
def test_rejects_unusable_remote_clipboard_without_overwriting_local(monkeypatch, stdout):
    written = []
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda *_args, **_kwargs: completed(stdout=stdout),
    )
    monkeypatch.setattr(clipboard, "write_text", lambda value: written.append(value) or True)

    with pytest.raises(typer.Exit):
        remote_clipboard.receive_from_archie()

    assert written == []


def test_remote_read_failure_does_not_overwrite_local_clipboard(monkeypatch):
    written = []
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda *_args, **_kwargs: completed(returncode=255, stderr=b"ssh failed\n"),
    )
    monkeypatch.setattr(clipboard, "write_text", lambda value: written.append(value) or True)

    with pytest.raises(typer.Exit):
        remote_clipboard.receive_from_archie()

    assert written == []


def test_local_write_failure_is_reported(monkeypatch, capsys):
    monkeypatch.setattr(
        remote_clipboard,
        "run",
        lambda *_args, **_kwargs: completed(stdout=b"hello"),
    )
    monkeypatch.setattr(clipboard, "write_text", lambda _text: False)

    with pytest.raises(typer.Exit):
        remote_clipboard.receive_from_archie()

    assert "could not write the local clipboard" in capsys.readouterr().err


def test_cpa_sensitive_flag_and_cpas_command_select_sensitive_mode(monkeypatch):
    calls = []
    monkeypatch.setattr(
        remote_clipboard,
        "send_to_archie",
        lambda sensitive=False, prog="cpa": calls.append((sensitive, prog)),
    )

    cpa_result = CliRunner().invoke(remote_clipboard.cpa_app, ["--sensitive"])
    cpas_result = CliRunner().invoke(remote_clipboard.cpas_app)

    assert cpa_result.exit_code == 0
    assert cpas_result.exit_code == 0
    assert calls == [(True, "cpa"), (True, "cpas")]


def test_missing_ssh_is_reported(monkeypatch, capsys):
    monkeypatch.setattr(clipboard, "read_text", lambda: "hello")

    def missing_ssh(*_args, **_kwargs):
        raise FileNotFoundError("ssh")

    monkeypatch.setattr(remote_clipboard, "run", missing_ssh)

    with pytest.raises(typer.Exit):
        remote_clipboard.send_to_archie()

    assert "could not start ssh" in capsys.readouterr().err
