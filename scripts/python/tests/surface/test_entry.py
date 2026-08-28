"""The flag as an installed command answers it, which is the whole contract."""

import pytest

from tools.surface import entry

PROGRAMS = sorted(entry.programs())


@pytest.mark.parametrize("program", PROGRAMS)
def test_every_command_prints_a_completion_script(tool, program):
    result = tool(program, "--completions", "zsh")
    assert result.returncode == 0
    assert result.stdout.startswith(f"#compdef {program}\n")


def test_the_flag_answers_before_a_missing_argument_does(tool):
    # tardirs needs an archive; --completions has to win anyway, as --help does.
    assert tool("tardirs").returncode != 0
    assert tool("tardirs", "--completions", "zsh").returncode == 0


def test_a_shell_with_no_script_says_so(tool):
    result = tool("dotfile", "--completions", "bash")
    assert result.returncode == 2
    assert "no bash completions" in result.stderr


def test_the_value_command_answers_the_generated_script(tool):
    result = tool("dotfile", "__complete", "theme-profiles")
    assert result.returncode == 0
    assert "mocha" in result.stdout.split()


def test_an_unknown_value_source_is_silent(tool):
    result = tool("dotfile", "__complete", "no-such-source")
    assert result.returncode == 0
    assert result.stdout == ""


def test_writing_every_script_leaves_one_file_to_source(tool, tmp_path):
    result = tool("dotfile", "completions", "--dir", str(tmp_path))
    assert result.returncode == 0
    assert (tmp_path / "tools-completion.zsh").is_file()


def test_the_documentation_matches_the_tools(tool):
    assert tool("dotfile", "docs", "--check").returncode == 0
