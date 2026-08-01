from types import SimpleNamespace

from tools.utils import oc


def test_reuses_a_listening_tunnel(monkeypatch):
    calls = []
    monkeypatch.setattr(oc, "capture", lambda command: SimpleNamespace(stdout="LISTEN 0 128\n"))
    monkeypatch.setattr(oc, "run", lambda command: calls.append(command))
    oc.ensure_tunnel()
    assert calls == []


def test_opens_the_tunnel_when_missing(monkeypatch):
    calls = []
    monkeypatch.setattr(oc, "capture", lambda command: SimpleNamespace(stdout=""))
    monkeypatch.setattr(oc, "run", lambda command: calls.append(command))
    oc.ensure_tunnel()
    assert len(calls) == 1
    assert calls[0][0] == "ssh"
    assert "-f" in calls[0]
    assert f"{oc.TUNNEL_PORT}:127.0.0.1:{oc.TUNNEL_PORT}" in calls[0]
    assert calls[0][-1] == oc.TUNNEL_HOST


def test_missing_ss_falls_back_to_ssh(monkeypatch):
    def missing(command):
        raise FileNotFoundError

    calls = []
    monkeypatch.setattr(oc, "capture", missing)
    monkeypatch.setattr(oc, "run", lambda command: calls.append(command))
    oc.ensure_tunnel()
    assert len(calls) == 1


def test_message_arguments_run_the_agent(monkeypatch):
    executed = []
    monkeypatch.setattr(oc, "ensure_tunnel", lambda: None)
    monkeypatch.setattr(oc.os, "execvp", lambda program, argv: executed.append((program, argv)))
    oc.oc(["hello", "world"])
    assert executed == [
        (
            "openclaw",
            ["openclaw", "agent", "--agent", "main", "--message", "hello world"],
        )
    ]


def test_no_arguments_open_the_tui(monkeypatch):
    executed = []
    monkeypatch.setattr(oc, "ensure_tunnel", lambda: None)
    monkeypatch.setattr(oc.os, "execvp", lambda program, argv: executed.append((program, argv)))
    oc.oc(None)
    assert executed == [("openclaw", ["openclaw", "tui"])]
