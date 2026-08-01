from types import SimpleNamespace

import pytest

from tools.desktop import power_menu


def select(monkeypatch, stdout):
    calls = []
    monkeypatch.setattr(
        power_menu,
        "capture",
        lambda command, input: SimpleNamespace(stdout=stdout),
    )
    monkeypatch.setattr(power_menu, "run", lambda command: calls.append(command))
    power_menu.power_menu()
    return calls


@pytest.mark.parametrize(
    ("selection", "command"),
    [
        ("\U000f033e  Lock\n", ["hyprlock"]),
        ("\U000f0343  Logout\n", ["hyprctl", "dispatch", "exit"]),
        ("\U000f0904  Suspend\n", ["systemctl", "suspend"]),
        ("\U000f0453  Reboot\n", ["systemctl", "reboot"]),
        ("\U000f0425  Shutdown\n", ["systemctl", "poweroff"]),
    ],
)
def test_runs_the_selected_action(monkeypatch, selection, command):
    assert select(monkeypatch, selection) == [command]


def test_dismissed_menu_does_nothing(monkeypatch):
    assert select(monkeypatch, "") == []


def test_unknown_selection_does_nothing(monkeypatch):
    assert select(monkeypatch, "something else\n") == []
