import subprocess
from pathlib import Path

from tools.dmux_rollout.model import Release


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def pushed_repo(root: Path, name: str) -> Path:
    remote = root / f"{name}.git"
    repo = root / name
    subprocess.run(["git", "init", "--bare", str(remote)], check=True, capture_output=True)
    subprocess.run(["git", "init", "-b", "main", str(repo)], check=True, capture_output=True)
    git(repo, "config", "user.name", "Rollout Test")
    git(repo, "config", "user.email", "rollout@example.invalid")
    (repo / "source.txt").write_text("one\n", encoding="utf-8")
    git(repo, "add", "source.txt")
    git(repo, "commit", "-m", "initial")
    git(repo, "remote", "add", "origin", str(remote))
    git(repo, "push", "-u", "origin", "main")
    return repo


def source(repo: Path, commit: str) -> dict:
    return {
        "repo": str(repo),
        "commit": commit,
        "origin": str(repo),
        "remote_refs": ["origin/main"],
        "main_worktree_dirty": [],
    }


def release(tmp_path: Path) -> Release:
    commit = "1" * 40
    return Release.create(
        release_id="20260817-test",
        dotfiles=source(tmp_path / "dotfiles", commit),
        wezterm=source(tmp_path / "wezterm", "2" * 40),
        smoke_name="rollout-smoke",
        archie_host="archie",
    )
