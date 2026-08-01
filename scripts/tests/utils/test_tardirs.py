import subprocess

import pytest


@pytest.fixture
def archive(tmp_path):
    root = tmp_path / "arch"
    (root / "a" / "b").mkdir(parents=True)
    (root / "a" / "c").mkdir(parents=True)
    (root / "d").mkdir()
    for name in ("a/b/f1", "a/b/f2", "a/c/f3", "d/f4", "top"):
        (root / name).touch()
    target = tmp_path / "test.tar.gz"
    subprocess.run(["tar", "czf", str(target), "-C", str(tmp_path), "arch"], check=True)
    return target


def test_prints_sorted_tree_with_direct_entry_counts(tool, archive):
    result = tool("tardirs", str(archive))
    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert lines[0] == "Archive directory tree"
    assert lines[1] == "count = direct archive entries mapped to that directory"
    assert lines[2] == ""
    assert lines[3] == "└─ arch/  [2]"
    assert "   ├─ a/  [1]" in lines
    assert "   │  ├─ b/  [3]" in lines
    assert "   │  └─ c/  [2]" in lines
    assert "   └─ d/  [2]" in lines


def test_max_depth_limits_the_tree(tool, archive):
    result = tool("tardirs", str(archive), "2")
    assert "b/" not in result.stdout
    assert "a/" in result.stdout


def test_rejects_a_missing_archive(tool, tmp_path):
    result = tool("tardirs", str(tmp_path / "missing.tar.gz"))
    assert result.returncode == 1
    assert "File not found" in result.stderr


def test_rejects_an_unsupported_extension(tool, tmp_path):
    target = tmp_path / "archive.zip"
    target.touch()
    result = tool("tardirs", str(target))
    assert result.returncode == 1
    assert "Unsupported archive" in result.stderr
