import subprocess

import pytest


@pytest.fixture
def work(tmp_path):
    origin = tmp_path / "origin.git"
    subprocess.run(["git", "init", "-q", "--bare", str(origin)], check=True)
    repo = tmp_path / "work"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.email", "t@t"], check=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.name", "t"], check=True)
    (repo / "seed").write_text("seed\n")
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
    subprocess.run(["git", "-C", str(repo), "commit", "-q", "-m", "seed"], check=True)
    subprocess.run(["git", "-C", str(repo), "remote", "add", "origin", str(origin)], check=True)
    subprocess.run(["git", "-C", str(repo), "push", "-q", "-u", "origin", "HEAD"], check=True)
    return repo


def last_subject(repo):
    return subprocess.run(
        ["git", "-C", str(repo), "log", "-1", "--format=%s"],
        capture_output=True,
        check=False,
        text=True,
    ).stdout.strip()


def test_stages_commits_and_pushes(tool, work):
    (work / "new.txt").write_text("content\n")
    result = tool("gpp", "add", "the", "file", cwd=str(work))
    assert result.returncode == 0
    assert last_subject(work) == "add the file"
    remote = subprocess.run(
        ["git", "-C", str(work), "rev-parse", "@{u}", "HEAD"],
        capture_output=True,
        check=False,
        text=True,
    ).stdout.split()
    assert len(remote) == 2
    assert remote[0] == remote[1]


def test_nothing_to_commit_fails(tool, work):
    result = tool("gpp", "empty", cwd=str(work))
    assert result.returncode == 1
    assert "nothing to commit" in result.stderr


def test_requires_a_message(tool, work):
    result = tool("gpp", cwd=str(work))
    assert result.returncode != 0
