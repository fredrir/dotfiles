import pytest


@pytest.fixture
def tree(tmp_path):
    (tmp_path / "a").touch()
    (tmp_path / ".b").touch()
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "c").touch()
    (tmp_path / "sub" / ".hid").mkdir()
    (tmp_path / "sub" / ".hid" / "d").touch()
    (tmp_path / ".hidden").mkdir()
    (tmp_path / ".hidden" / "e").touch()
    return tmp_path


def test_counts_direct_children(tool, tree):
    result = tool("count", str(tree))
    assert result.returncode == 0
    assert result.stdout == "4\n"


def test_counts_recursively(tool, tree):
    result = tool("count", "-r", str(tree))
    assert result.stdout == "8\n"


def test_skips_hidden_children(tool, tree):
    result = tool("count", "-d", str(tree))
    assert result.stdout == "2\n"


def test_combined_flags_skip_hidden_recursively(tool, tree):
    result = tool("count", "-rd", str(tree))
    assert result.stdout == "3\n"


def test_rejects_a_file(tool, tree):
    result = tool("count", str(tree / "a"))
    assert result.returncode == 1
    assert "not a directory" in result.stderr


def test_requires_an_argument(tool):
    result = tool("count")
    assert result.returncode != 0
