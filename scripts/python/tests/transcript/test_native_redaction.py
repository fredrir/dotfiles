import os
import sys
import time

import pytest
from native import rust_binary

from tools.transcript import cli, native_redaction


@pytest.fixture
def native_redactor(tmp_path, monkeypatch):
    executable = str(rust_binary("dotfile-cli", "dotfile"))
    root = tmp_path / "repo"
    (root / "config").mkdir(parents=True)
    home = tmp_path / "home"
    state = home / ".config" / "dotfile"
    state.mkdir(parents=True)
    canaries = state / "canaries"
    canaries.write_text(
        "host = private-fixture.example\n"
        "unicode-host = prİvate-fixture.example\n"
        "overlap = prefix ghp_abcdefghijklmnopqrstuv1234567890 suffix\n"
    )
    canaries.chmod(0o600)
    monkeypatch.setenv("DOTFILE_ROOT", str(root))
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(home / ".config"))
    monkeypatch.setattr(native_redaction, "binary", lambda _: executable)
    redactor = native_redaction.Redactor()
    try:
        yield redactor
    finally:
        redactor._cleanup()


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        (
            "token ghp_abcdefghijklmnopqrstuv1234567890 done",
            "token [redacted:github-token] done",
        ),
        (
            "sk-abcdefghijklmnop1234 and AKIAABCDEFGHIJKLMNOP",
            "[redacted:api-key] and [redacted:aws-key]",
        ),
        ("export API_KEY=supersecretvalue123", "export API_KEY=[redacted:value]"),
        (
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----",
            "[redacted:private-key]",
        ),
        (
            "git commit -m 'update config' && git push origin main",
            "git commit -m 'update config' && git push origin main",
        ),
        (
            "box at PRIVATE-Fixture.Example and private-fixture.example",
            "box at [redacted:private] and [redacted:private]",
        ),
        (
            "İK at PRİVATE-fixture.example and pri\u0307vate-fixture.example",
            "İK at [redacted:private] and [redacted:private]",
        ),
        (
            "start prefix ghp_abcdefghijklmnopqrstuv1234567890 suffix end",
            "start [redacted:private] end",
        ),
    ],
)
def test_native_redaction_filters_tokens_and_private_values(native_redactor, text, expected):
    assert native_redactor(text) == expected


def test_raw_transcript_text_does_not_start_a_redaction_service(monkeypatch):
    def unexpected(_):
        raise AssertionError("raw text must not start the redaction service")

    monkeypatch.setattr(native_redaction, "binary", unexpected)
    text = "ghp_abcdefghijklmnopqrstuv1234567890 private-fixture.example"
    assert cli._redactor(True)(text) == text


def helper(tmp_path, monkeypatch, body):
    path = tmp_path / "helper"
    path.write_text(f"#!{sys.executable}\n{body}\n")
    path.chmod(0o755)
    monkeypatch.setattr(native_redaction, "binary", lambda _: str(path))
    monkeypatch.setattr(native_redaction, "TIMEOUT", 0.15)
    return native_redaction.Redactor()


def test_stalled_stdin_obeys_common_deadline_and_reaps_helper(tmp_path, monkeypatch):
    redactor = helper(tmp_path, monkeypatch, "import time; time.sleep(30)")
    started = time.monotonic()
    with pytest.raises(RuntimeError, match="native redaction failed"):
        redactor("x" * (512 * 1024))
    assert time.monotonic() - started < 2
    assert redactor.process.poll() is not None
    assert redactor.process.stdin.closed
    assert redactor.process.stdout.closed


def test_partial_stdout_obeys_deadline_and_reaps_helper(tmp_path, monkeypatch):
    redactor = helper(
        tmp_path,
        monkeypatch,
        "import sys,time; sys.stdout.write(chr(34)); sys.stdout.flush(); time.sleep(30)",
    )
    started = time.monotonic()
    with pytest.raises(RuntimeError, match="native redaction failed"):
        redactor("fixture")
    assert time.monotonic() - started < 2
    assert redactor.process.poll() is not None


def test_invalid_protocol_response_reaps_helper(tmp_path, monkeypatch):
    redactor = helper(
        tmp_path,
        monkeypatch,
        "import sys,time; sys.stdin.readline(); print(123,flush=True); time.sleep(30)",
    )
    with pytest.raises(RuntimeError, match="native redaction failed"):
        redactor("fixture")
    assert redactor.process.poll() is not None


def test_repeated_unicode_records_keep_protocol_boundaries(tmp_path, monkeypatch):
    redactor = helper(
        tmp_path,
        monkeypatch,
        "import sys,json\nfor line in sys.stdin:\n print(json.dumps(json.loads(line)),flush=True)",
    )
    assert redactor("Norsk æøå\nnext line") == "Norsk æøå\nnext line"
    assert redactor("日本語") == "日本語"
    redactor._cleanup()
    assert redactor.process.poll() is not None


@pytest.mark.skipif(os.name != "posix", reason="requires POSIX child-group cancellation")
def test_cleanup_cancels_initial_sops_decryption_without_orphan(tmp_path, monkeypatch):
    from tools.core.native import binary

    executable = binary("dotfile")
    root = tmp_path / "repo"
    (root / "config").mkdir(parents=True)
    (root / "vars.enc.yaml").write_text("encrypted placeholder")
    home = tmp_path / "home"
    state = home / ".config" / "dotfile"
    (state / "age").mkdir(parents=True)
    (state / "age" / "keys.txt").write_text("identity placeholder")
    tools = tmp_path / "tools"
    tools.mkdir()
    marker = tmp_path / "sops.pid"
    stub = tools / "sops"
    stub.write_text('#!/bin/sh\necho $$ > "$TEST_SOPS_PID"\nsleep 30\n')
    stub.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tools}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(home / ".config"))
    monkeypatch.setenv("DOTFILE_ROOT", str(root))
    monkeypatch.setenv("TEST_SOPS_PID", str(marker))
    monkeypatch.setattr(native_redaction, "binary", lambda _: executable)
    redactor = native_redaction.Redactor()
    deadline = time.monotonic() + 3
    while not marker.exists() and time.monotonic() < deadline:
        time.sleep(0.01)
    try:
        assert marker.exists(), "native SOPS child did not start"
        pid = int(marker.read_text())
        redactor._cleanup()
        assert redactor.process.poll() is not None
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
    finally:
        redactor._cleanup()
