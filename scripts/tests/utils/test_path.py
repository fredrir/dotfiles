import subprocess

import pytest


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / "repo"
    (root / "sub").mkdir(parents=True)
    (root / "sub" / "file.txt").touch()
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    return root


def test_repo_root_prints_slash(tool, repo):
    result = tool("path", cwd=str(repo))
    assert result.stdout == "/\n"


def test_repo_relative_path(tool, repo):
    result = tool("path", "sub/file.txt", cwd=str(repo))
    assert result.stdout == "/sub/file.txt\n"


def test_nonexistent_target_inside_repo(tool, repo):
    result = tool("path", "missing/deep.txt", cwd=str(repo))
    assert result.stdout == "/missing/deep.txt\n"


def test_home_relative_outside_repo(tool, tmp_path):
    home = tmp_path / "home"
    (home / "docs").mkdir(parents=True)
    env = {"HOME": str(home)}
    result = tool("path", cwd=str(home), env=env)
    assert result.stdout == "~\n"
    result = tool("path", "docs", cwd=str(home), env=env)
    assert result.stdout == "~/docs\n"


def test_absolute_path_outside_repo_and_home(tool, tmp_path):
    env = {"HOME": str(tmp_path / "nowhere")}
    result = tool("path", "/usr/share", env=env)
    assert result.stdout == "/usr/share\n"


def test_rejects_extra_arguments(tool):
    result = tool("path", "a", "b")
    assert result.returncode == 2
