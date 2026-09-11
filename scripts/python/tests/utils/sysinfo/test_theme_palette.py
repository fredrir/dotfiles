import json
from subprocess import CompletedProcess, TimeoutExpired

import pytest

from tools.utils.sysinfo import pretty


def document():
    return {
        "version": 1,
        "colors": {
            name: "#112233" for name in ("fg", "muted", "separator", "green", "yellow", "red")
        },
        "roles": {
            name: "#445566" for name in ("section_system", "section_hardware", "section_desktop")
        },
    }


def test_colors_come_from_native_palette_and_are_refreshed(monkeypatch, tmp_path):
    monkeypatch.setattr(pretty, "binary", lambda name: f"/native/{name}")
    monkeypatch.setattr(pretty, "dotfiles_root", lambda: tmp_path)
    values = document()
    calls = []

    def capture(command, **kwargs):
        calls.append((command, kwargs))
        return CompletedProcess(command, 0, json.dumps(values), "")

    monkeypatch.setattr(pretty, "capture", capture)
    assert pretty.load_colors().text == "#112233"
    values["colors"]["fg"] = "#abcdef"
    assert pretty.load_colors().text == "#abcdef"
    assert (
        calls
        == [(["/native/dotfile", "theme", "palette", "--json"], {"cwd": tmp_path, "timeout": 10})]
        * 2
    )


@pytest.mark.parametrize("failure", ["exit", "json", "version", "color", "missing", "timeout"])
def test_native_palette_failures_are_actionable(monkeypatch, failure):
    monkeypatch.setattr(pretty, "binary", lambda _name: "/native/dotfile")
    values = document()
    if failure == "version":
        values["version"] = 2
    if failure == "color":
        values["colors"]["fg"] = "not-a-color"
    if failure == "missing":
        del values["roles"]

    def capture(command, **_kwargs):
        if failure == "timeout":
            raise TimeoutExpired(command, 10)
        return CompletedProcess(
            command,
            1 if failure == "exit" else 0,
            "bad-json" if failure == "json" else json.dumps(values),
            "bad theme" if failure == "exit" else "",
        )

    monkeypatch.setattr(pretty, "capture", capture)
    with pytest.raises(SystemExit, match="sysinfo: theme palette:"):
        pretty.load_colors()
