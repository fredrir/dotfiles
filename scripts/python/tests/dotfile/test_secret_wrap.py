import io
import os
import stat
import subprocess

import pexpect
import pytest
from native import rust_binary

PASSPHRASE = "correct horse battery staple"
OTHER_KEY = "age1" + "q" * 58


@pytest.fixture
def machine(tool, tmp_path):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    (root / "config").mkdir(parents=True)
    (root / "environment" / "test").mkdir(parents=True)
    (root / "environment" / "test" / "manifest").write_text("shared\n")
    (root / "shared").mkdir()
    (root / "config" / "targets.dotfile").write_text("")
    (root / "config" / "profile").write_text("test\n")
    (root / "config" / "hosts.dotfile").write_text("box {\n  hostnames = box\n}\n")
    (root / ".gitignore").write_text("config/age/keys.txt\n")
    (home / ".config").mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    env = {
        "DOTFILE_ROOT": str(root),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "SYSINFO_HOST": "box",
        "NO_COLOR": "1",
    }
    assert tool("dotfile", "secret", "init", env=env).returncode == 0
    assert tool("dotfile", "secret", "enroll", "box", env=env).returncode == 0
    return root, env


def spawn(env, *args):
    child = pexpect.spawn(
        str(rust_binary("dotfile-cli", "dotfile")),
        list(args),
        env=dict(os.environ, **env),
        encoding="utf-8",
        timeout=60,
    )
    child.logfile_read = io.StringIO()
    return child


def answer(child, *passphrases):
    for passphrase in passphrases:
        child.expect("passphrase")
        child.sendline(passphrase)


def finish(child):
    child.expect(pexpect.EOF)
    child.close()
    return child.exitstatus, child.logfile_read.getvalue()


def wrap(env, *passphrases):
    child = spawn(env, "secret", "wrap")
    answer(child, *passphrases)
    return finish(child)


def unwrap(env, *passphrases):
    child = spawn(env, "secret", "unwrap")
    answer(child, *passphrases)
    return finish(child)


def identity(root):
    return root / "config" / "age" / "keys.txt"


def wrapped(root):
    return root / "config" / "age" / "box.age"


def test_a_wrapped_identity_restores_byte_for_byte(machine):
    root, env = machine
    original = identity(root).read_bytes()
    status, output = wrap(env, PASSPHRASE, PASSPHRASE)
    assert status == 0, output
    assert wrapped(root).read_text().startswith("-----BEGIN AGE ENCRYPTED FILE-----")
    identity(root).unlink()
    status, output = unwrap(env, PASSPHRASE)
    assert status == 0, output
    assert identity(root).read_bytes() == original
    assert stat.S_IMODE(identity(root).stat().st_mode) == 0o600


def test_wrapping_stages_the_sealed_copy(machine):
    root, env = machine
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    staged = subprocess.run(
        ["git", "-C", str(root), "diff", "--cached", "--name-only"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    assert "config/age/box.age" in staged


def test_a_wrong_passphrase_can_be_retried(machine):
    root, env = machine
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    identity(root).unlink()
    status, output = unwrap(env, "not the passphrase", PASSPHRASE)
    assert status == 0, output
    assert "wrong passphrase, try again" in output
    assert identity(root).is_file()


def test_three_wrong_passphrases_write_nothing(machine):
    root, env = machine
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    identity(root).unlink()
    status, output = unwrap(env, "wrong one", "wrong two", "wrong three")
    assert status == 1
    assert "wrong passphrase 3 times" in output
    assert not identity(root).exists()


def test_a_short_passphrase_is_refused(machine):
    root, env = machine
    status, output = wrap(env, "short")
    assert status == 1
    assert "passphrase under 12 characters" in output
    assert not wrapped(root).exists()


def test_differing_repeats_are_refused(machine):
    root, env = machine
    status, output = wrap(env, PASSPHRASE, PASSPHRASE + "!")
    assert status == 1
    assert "the passphrases differ" in output
    assert not wrapped(root).exists()


def test_only_an_enrolled_identity_is_wrapped(tool, machine):
    root, env = machine
    (root / "config" / "keys.dotfile").write_text(f"recipients {{\n  box = {OTHER_KEY}\n}}\n")
    result = tool("dotfile", "secret", "wrap", env=env)
    assert result.returncode == 1
    assert "not enrolled as 'box'" in result.stderr
    assert not wrapped(root).exists()


def test_a_wrap_of_a_rolled_away_key_is_not_restored(machine):
    root, env = machine
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    identity(root).unlink()
    (root / "config" / "keys.dotfile").write_text(f"recipients {{\n  box = {OTHER_KEY}\n}}\n")
    status, output = unwrap(env, PASSPHRASE)
    assert status == 1
    assert "not enrolled as 'box'" in output
    assert not identity(root).exists()


def test_unwrap_never_replaces_an_identity(tool, machine):
    root, env = machine
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    before = identity(root).read_bytes()
    result = tool("dotfile", "secret", "unwrap", env=env)
    assert result.returncode == 1
    assert "already exists" in result.stderr
    assert identity(root).read_bytes() == before


def test_unwrap_without_a_wrapped_copy_says_how_to_make_one(tool, machine):
    root, env = machine
    identity(root).unlink()
    result = tool("dotfile", "secret", "unwrap", env=env)
    assert result.returncode == 1
    assert "dotfile secret wrap" in result.stderr


def test_sync_restores_a_missing_identity_before_reconciling(machine):
    root, env = machine
    original = identity(root).read_bytes()
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    identity(root).unlink()
    child = spawn(env, "sync", "test")
    answer(child, PASSPHRASE)
    status, output = finish(child)
    assert status == 0, output
    assert identity(root).read_bytes() == original


def test_doctor_notes_whether_a_reinstall_can_restore(tool, machine):
    env = machine[1]
    before = tool("dotfile", "secret", "doctor", env=env).stdout
    assert "not wrapped; run dotfile secret wrap" in before
    assert wrap(env, PASSPHRASE, PASSPHRASE)[0] == 0
    after = tool("dotfile", "secret", "doctor", env=env).stdout
    assert "wrapped      config/age/box.age" in after
