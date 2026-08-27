"""The generators' side of `dotfmt`: it formats when it can, and never loses text."""

import os

import pytest

from tools.core.dotfmt import formatted


@pytest.fixture
def stub(tmp_path, monkeypatch):
    def build(script):
        path = tmp_path / "dotfmt"
        path.write_text(f"#!/bin/sh\n{script}\n")
        path.chmod(0o755)
        monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
        return path

    return build


def test_the_text_comes_back_through_the_formatter(stub):
    stub("sed 's/=/ =/'")
    assert formatted("a= 1\n", "x.dotfile") == "a = 1\n"


def test_the_filename_is_passed_along_so_the_mode_can_be_chosen(stub):
    stub('printf "%s" "$2"')
    assert formatted("body\n", "/tmp/hyprland.conf") == "/tmp/hyprland.conf"


def test_a_formatter_that_is_not_installed_leaves_the_text_alone(monkeypatch, tmp_path):
    # setup.sh builds dotfmt, and runs the generators on the way there.
    monkeypatch.setenv("PATH", str(tmp_path))
    assert formatted("a = 1\n", "x.dotfile") == "a = 1\n"


def test_a_formatter_that_fails_leaves_the_text_alone(stub):
    stub("cat >/dev/null; exit 1")
    assert formatted("a = 1\n", "x.dotfile") == "a = 1\n"


def test_a_formatter_that_returns_nothing_leaves_the_text_alone(stub):
    stub("cat >/dev/null")
    assert formatted("a = 1\n", "x.dotfile") == "a = 1\n"


def test_nothing_to_format_never_starts_a_process(monkeypatch):
    monkeypatch.setenv("PATH", os.devnull)
    assert formatted("", "x.dotfile") == ""
