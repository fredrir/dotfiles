import json

import typer.main
from native import ROOT

from tools.transcript import cli, completion


def test_declarative_commands_match_the_python_parser():
    tree = json.loads((ROOT / "config/command-surface.json").read_text())["commands"]["transcript"]

    def check(command, declared):
        params = {param.name: param for param in command.params}
        for param in declared["params"]:
            assert param["name"] in params
            actual = params[param["name"]]
            if param["kind"] == "option":
                assert set(param["opts"]) == set(actual.opts)
            assert param["required"] == actual.required
        expected = {child["path"][-1]: child for child in declared["children"] if not child["hidden"]}
        actual = {name: child for name, child in getattr(command, "commands", {}).items() if not child.hidden}
        assert expected.keys() == actual.keys()
        for name, child in actual.items():
            check(child, expected[name])

    check(typer.main.get_command(cli.app), tree)


def test_dynamic_completions_are_owned_by_transcript(monkeypatch):
    monkeypatch.setattr(completion, "values", lambda source, args: [("a:b", "two\nlines")])
    assert completion.lines("sessions", []) == [r"a\:b:two lines"]


def test_unavailable_completion_source_stays_quiet(monkeypatch):
    def failed(*args):
        raise OSError("unavailable")

    monkeypatch.setattr(completion, "values", failed)
    assert completion.lines("sessions", []) == []


def test_installed_help_works_outside_repository(tool, tmp_path):
    for program in ["transcript", "clean-copy"]:
        result = tool(program, "--help", cwd=tmp_path)
        assert result.returncode == 0, result.stderr
        assert f"Usage: {program}" in result.stdout
