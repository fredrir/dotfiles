from types import SimpleNamespace

from tools.desktop import confirm_exit


def select(monkeypatch, stdout):
    calls = []
    monkeypatch.setattr(
        confirm_exit,
        "capture",
        lambda command, input: SimpleNamespace(stdout=stdout),
    )
    monkeypatch.setattr(confirm_exit, "run", lambda command: calls.append(command))
    confirm_exit.confirm_exit()
    return calls


def test_yes_exits_hyprland(monkeypatch):
    assert select(monkeypatch, "Yes\n") == [["hyprctl", "dispatch", "exit"]]


def test_no_does_nothing(monkeypatch):
    assert select(monkeypatch, "No\n") == []


def test_dismissed_menu_does_nothing(monkeypatch):
    assert select(monkeypatch, "") == []
