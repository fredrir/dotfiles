"""`dotfile sync --push`, exercised against a recorded stand-in for ssh.

The far side is the part that cannot be tested for real, so `ssh` is replaced
by a script that logs every invocation and answers the status probe from a
file. What the tests then assert is the thing that matters: which commands
would have crossed the wire, and in which order.
"""

import os

import pytest
from gitrepo import run_git

CLEAN = "## main...origin/main\n"
DIRTY = "## main...origin/main\n M shared/alpha/alpha.conf\n"

HOSTS = """\
archie {
  hostnames = archpc, archie
  role = desktop
}

macie {
  hostnames = macie
  role = laptop
}
"""

FAKE_SSH = """\
#!/bin/sh
{ printf '%s\\n' "$*"; printf '===\\n'; } >> "$FAKE_SSH_LOG"
case "$*" in
  *'git status --porcelain --branch'*) cat "$FAKE_SSH_STATUS" ;;
esac
exit "${FAKE_SSH_EXIT:-0}"
"""


class Remote:
    """The recorded far side: what it answers, and what it was asked."""

    def __init__(self, log, status):
        self.log = log
        self.status = status

    def answers(self, text):
        self.status.write_text(text)

    def calls(self):
        if not self.log.exists():
            return []
        return [call.strip() for call in self.log.read_text().split("===\n") if call.strip()]

    def asked(self, fragment):
        return any(fragment in call for call in self.calls())


@pytest.fixture
def machine(tmp_path):
    home = tmp_path / "home"
    repo = home / "dotfiles"
    origin = tmp_path / "origin.git"
    binaries = tmp_path / "bin"
    (home / ".config").mkdir(parents=True)
    (repo / "shared" / "alpha").mkdir(parents=True)
    (repo / "shared" / "alpha" / "alpha.conf").write_text("alpha\n")
    (repo / "environment" / "test").mkdir(parents=True)
    (repo / "environment" / "test" / "manifest").write_text("shared\n")
    (repo / "config").mkdir()
    (repo / "config" / "targets.dotfile").write_text("")
    (repo / "config" / "hosts.dotfile").write_text(HOSTS)
    setup = repo / "setup.sh"
    setup.write_text('#!/bin/sh\nprintf "%s" "$*" > "$(dirname "$0")/called.txt"\nexit 0\n')
    setup.chmod(0o755)

    binaries.mkdir()
    ssh = binaries / "ssh"
    ssh.write_text(FAKE_SSH)
    ssh.chmod(0o755)

    run_git(tmp_path, "init", "-q", "--bare", "-b", "main", str(origin))
    run_git(tmp_path, "init", "-q", "-b", "main", str(repo))
    run_git(repo, "config", "user.email", "test@example.com")
    run_git(repo, "config", "user.name", "test")
    run_git(repo, "remote", "add", "origin", str(origin))
    run_git(repo, "add", "-A")
    run_git(repo, "commit", "-qm", "first")
    run_git(repo, "push", "-q", "-u", "origin", "main")

    remote = Remote(tmp_path / "ssh.log", tmp_path / "status.txt")
    remote.answers(CLEAN)
    env = {
        "DOTFILE_ROOT": str(repo),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "SYSINFO_HOST": "macie",
        "PATH": f"{binaries}:{os.environ['PATH']}",
        "FAKE_SSH_LOG": str(remote.log),
        "FAKE_SSH_STATUS": str(remote.status),
    }
    return repo, env, remote


def test_push_pulls_and_syncs_the_other_machine(tool, machine):
    repo, env, remote = machine
    result = tool("dotfile", "sync", "-p", env=env)
    assert result.returncode == 0
    assert (repo / "called.txt").read_text() == "--sync"
    assert "nothing to push" in result.stdout
    assert remote.asked('cd "$HOME"/dotfiles')
    assert remote.asked("git pull || exit 1\ndotfile sync")
    assert remote.calls()[-1].startswith("-t archie")


def test_push_resolves_the_peer_from_the_host_file(tool, machine):
    _repo, env, remote = machine
    assert tool("dotfile", "sync", "-p", env=dict(env, SYSINFO_HOST="archie")).returncode == 0
    assert remote.calls()[-1].startswith("-t macie")


def test_push_accepts_a_named_machine(tool, machine):
    _repo, env, remote = machine
    assert tool("dotfile", "sync", "-p", "--to", "archie", env=env).returncode == 0
    assert remote.calls()[-1].startswith("-t archie")


def test_push_sends_the_commits_that_are_ahead(tool, machine):
    repo, env, remote = machine
    (repo / "shared" / "alpha" / "alpha.conf").write_text("beta\n")
    run_git(repo, "commit", "-qam", "second")
    result = tool("dotfile", "sync", "-p", env=env)
    assert result.returncode == 0
    assert "pushing 1 commit to origin/main" in result.stdout
    assert remote.asked("git pull")


def test_push_forwards_the_resolution_to_the_other_machine(tool, machine):
    _repo, env, remote = machine
    assert tool("dotfile", "sync", "-p", "--force", env=env).returncode == 0
    assert remote.asked("dotfile sync --resolve repo")


def test_push_discards_the_far_working_tree_with_force(tool, machine):
    _repo, env, remote = machine
    remote.answers(DIRTY)
    result = tool("dotfile", "sync", "-p", "--force", env=env)
    assert result.returncode == 0
    assert "M shared/alpha/alpha.conf" in result.stdout
    assert remote.asked("git reset --hard\ngit clean -fd")


def test_push_refuses_a_dirty_far_tree_without_a_tty(tool, machine):
    _repo, env, remote = machine
    remote.answers(DIRTY)
    result = tool("dotfile", "sync", "-p", env=env)
    assert result.returncode == 1
    assert "rerun with --force" in result.stderr
    assert not remote.asked("git reset --hard")
    assert not remote.asked("dotfile sync")


def test_push_stops_when_the_machines_are_on_different_branches(tool, machine):
    _repo, env, remote = machine
    remote.answers("## other\n")
    result = tool("dotfile", "sync", "-p", env=env)
    assert result.returncode == 1
    assert "archie is on 'other' but this machine is on 'main'" in result.stderr
    assert not remote.asked("dotfile sync")


def test_push_reports_the_far_exit_status(tool, machine):
    _repo, env, _remote = machine
    result = tool("dotfile", "sync", "-p", env=dict(env, FAKE_SSH_EXIT="3"))
    assert result.returncode == 1
    assert "cannot read the repository there" in result.stderr


def test_push_rejects_an_unknown_machine_before_syncing(tool, machine):
    repo, env, _remote = machine
    result = tool("dotfile", "sync", "-p", "--to", "nosuch", env=env)
    assert result.returncode == 1
    assert "unknown machine 'nosuch'" in result.stderr
    assert "archie, macie" in result.stderr
    assert not (repo / "called.txt").exists()


def test_push_refuses_to_target_this_machine(tool, machine):
    _repo, env, _remote = machine
    result = tool("dotfile", "sync", "--to", "macie", env=env)
    assert result.returncode == 1
    assert "is this machine" in result.stderr


def test_dry_run_leaves_the_other_machine_alone(tool, machine):
    _repo, env, remote = machine
    result = tool("dotfile", "sync", "-p", "-n", env=env)
    assert result.returncode == 0
    assert "would push 'main'" in result.stdout
    assert remote.calls() == []
