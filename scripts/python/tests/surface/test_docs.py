"""Reference output is owned and checked by the native command."""


def test_repository_reference_matches_command_parsers(tool):
    result = tool("dotfile", "__reference", "--check")
    assert result.returncode == 0, result.stdout + result.stderr
