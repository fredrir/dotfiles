import os
import shutil
import stat
import subprocess

import onepassword
import pytest

KEY_A = "age1" + "q" * 58
KEY_B = "age1" + "p" * 58
ALIGNED = f"recipients {{\n  archie   = {KEY_A}\n  recovery = {KEY_B}\n}}"


def run_git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True)


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    root.mkdir()
    (root / "config").mkdir()
    (home / ".config" / "dotfile").mkdir(parents=True)
    run_git(tmp_path, "init", "-q", str(root))
    run_git(root, "config", "user.email", "test@example.com")
    run_git(root, "config", "user.name", "test")
    env = {
        "DOTFILE_ROOT": str(root),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
    }
    return root, home, env


def with_identity(repo, tmp_path):
    """This machine gets a 1Password item reference and the fake op."""
    root, _home, env = repo
    onepassword.declare_host(root)
    env.update(onepassword.install(tmp_path))
    return env


def write_keys(root, text):
    (root / "config" / "keys.dotfile").write_text(text)


def stat_mode(path):
    return stat.S_IMODE(os.stat(path).st_mode)


def secret(tool, env, *args):
    return tool("dotfile", "secret", *args, env=env)


def test_enroll_writes_both_files(tool, repo):
    root, _home, env = repo
    assert secret(tool, env, "enroll", "archie", KEY_A).returncode == 0
    assert (
        root / "config" / "keys.dotfile"
    ).read_text() == f"recipients {{\n  archie = {KEY_A}\n}}\n"
    assert KEY_A in (root / ".sops.yaml").read_text()


def test_enroll_is_idempotent(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "enroll", "archie", KEY_A)
    assert result.returncode == 0
    assert "already enrolled" in result.stdout


def test_enroll_refuses_a_key_held_under_another_label(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "enroll", "laptop", KEY_A)
    assert result.returncode == 1
    assert "already enrolled as 'archie'" in result.stderr


def test_enroll_refuses_to_replace_a_label(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "enroll", "archie", KEY_B)
    assert result.returncode == 1
    assert "revoke it first" in result.stderr


def test_revoke_removes_and_regenerates(tool, repo):
    root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    secret(tool, env, "enroll", "recovery", KEY_B)
    assert secret(tool, env, "revoke", "archie").returncode == 0
    assert (
        root / "config" / "keys.dotfile"
    ).read_text() == f"recipients {{\n  recovery = {KEY_B}\n}}\n"
    assert KEY_A not in (root / ".sops.yaml").read_text()


def test_revoke_refuses_the_last_recipient(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "revoke", "archie")
    assert result.returncode == 1
    assert "only recipient" in result.stderr


def test_revoke_warns_that_rewrapping_is_not_rotation(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    secret(tool, env, "enroll", "recovery", KEY_B)
    result = secret(tool, env, "revoke", "archie")
    assert "rotate anything that key actually protected" in result.stdout


def test_sync_rewrites_a_drifted_sops_file(tool, repo):
    root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    (root / ".sops.yaml").write_text("creation_rules: []\n")
    assert (root / ".sops.yaml").read_text() != f"creation_rules:\n  - age: {KEY_A}\n"
    assert secret(tool, env, "sync").returncode == 0
    assert (root / ".sops.yaml").read_text() == f"creation_rules:\n  - age: {KEY_A}\n"


def test_sync_leaves_a_formatted_file_byte_for_byte(tool, repo):
    root, _home, env = repo
    write_keys(root, ALIGNED)
    assert secret(tool, env, "sync").returncode == 0
    assert (root / "config" / "keys.dotfile").read_text() == ALIGNED
    assert KEY_A in (root / ".sops.yaml").read_text()


def test_a_changed_recipient_uses_stable_formatting(tool, repo):
    root, _home, env = repo
    write_keys(root, ALIGNED)
    assert secret(tool, env, "revoke", "recovery").returncode == 0
    assert (root / "config" / "keys.dotfile").read_text() == (
        f"recipients {{\n  archie = {KEY_A}\n}}\n"
    )
    assert KEY_B not in (root / ".sops.yaml").read_text()


def test_sync_without_recipients_fails(tool, repo):
    _root, _home, env = repo
    result = secret(tool, env, "sync")
    assert result.returncode == 1
    assert "no recipients" in result.stderr


def test_keys_lists_what_is_enrolled(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "keys")
    assert "archie" in result.stdout
    assert KEY_A in result.stdout


def test_doctor_fails_on_a_fresh_repository(tool, repo):
    _root, _home, env = repo
    result = secret(tool, env, "doctor")
    assert result.returncode == 1
    assert "identity" in result.stdout
    assert "recipients" in result.stdout


def test_doctor_needs_no_recovery_recipient(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "doctor")
    recipients = next(line for line in result.stdout.splitlines() if "recipients" in line)
    assert recipients.split()[0] == "ok"
    assert "1 enrolled" in recipients


def test_enroll_stages_what_it_changed(tool, repo):
    root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    staged = subprocess.run(
        ["git", "-C", str(root), "diff", "--cached", "--name-only"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    assert "config/keys.dotfile" in staged
    assert ".sops.yaml" in staged


def test_a_new_machine_is_told_what_to_run_elsewhere(tool, repo, tmp_path):
    env = with_identity(repo, tmp_path)
    assert secret(tool, env, "init").returncode == 0
    secret(tool, env, "enroll", "other", KEY_A)
    result = secret(tool, env, "doctor")
    assert result.returncode == 1
    assert "not a recipient yet" in result.stdout
    assert "machine that is enrolled" in result.stdout
    assert "dotfile secret enroll" in result.stdout


def test_a_machine_that_is_a_recipient_says_so(tool, repo, tmp_path):
    env = with_identity(repo, tmp_path)
    assert secret(tool, env, "init").returncode == 0
    assert secret(tool, env, "enroll", onepassword.HOST).returncode == 0
    result = secret(tool, env, "doctor")
    assert f"this machine is '{onepassword.HOST}'" in result.stdout
    assert onepassword.field(env, "username") in (repo[0] / "config" / "keys.dotfile").read_text()


def test_init_puts_the_new_key_in_1password_and_the_shared_key_file(tool, repo, tmp_path):
    _root, home, _env = repo
    env = with_identity(repo, tmp_path)
    result = secret(tool, env, "init")
    assert result.returncode == 0, result.stderr
    public = onepassword.field(env, "username")
    assert public in result.stdout
    shared = home / ".config" / "sops" / "age" / "keys.txt"
    assert shared.read_text().strip() == onepassword.field(env, "credential")
    assert stat_mode(shared) == 0o600
    again = secret(tool, env, "init")
    assert again.returncode == 1
    assert "already exists" in again.stderr


@pytest.mark.skipif(
    not (shutil.which("sops") and shutil.which("age-keygen")), reason="needs age and sops"
)
def test_a_stranded_machine_enrols_itself_with_the_recovery_key(tool, repo, tmp_path):
    root, _home, _env = repo
    env = with_identity(repo, tmp_path)
    (root / "environment" / "test").mkdir(parents=True)
    (root / "environment" / "test" / "manifest").write_text("shared\n")
    (root / "shared").mkdir()
    (root / "config" / "targets.dotfile").write_text("")
    (root / "config" / "profile").write_text("test\n")

    recovery = tmp_path / "recovery.txt"
    subprocess.run(["age-keygen", "-o", str(recovery)], check=True, capture_output=True)
    pub = subprocess.run(
        ["age-keygen", "-y", str(recovery)], check=True, capture_output=True, text=True
    ).stdout.strip()
    assert secret(tool, env, "enroll", "recovery", pub).returncode == 0

    plain = tmp_path / "plain.yaml"
    plain.write_text("hosts:\n  demo: 203.0.113.55\n")
    sealed = subprocess.run(
        ["sops", "--config", str(root / ".sops.yaml"), "-e", str(plain)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    (root / "vars.enc.yaml").write_text(sealed)
    run_git(root, "add", "-A")

    assert secret(tool, env, "init").returncode == 0
    assert secret(tool, env, "vars").returncode == 1

    result = secret(tool, env, "enroll", onepassword.HOST, "--using", str(recovery))
    assert result.returncode == 0, result.stderr
    assert "re-wrapped 1 of 1 file" in result.stdout

    listed = secret(tool, env, "vars")
    assert listed.returncode == 0, listed.stderr
    assert "hosts.demo" in listed.stdout


def test_using_rejects_a_path_that_is_not_an_identity(tool, repo, tmp_path):
    _root, _home, env = repo
    secret(tool, env, "enroll", "other", KEY_A)
    junk = tmp_path / "junk.txt"
    junk.write_text("not a key\n")
    result = secret(tool, env, "enroll", "new", KEY_B, "--using", str(junk))
    assert result.returncode == 1
    assert "not readable as an age identity" in result.stderr

    result = secret(tool, env, "enroll", "new", KEY_B, "--using", str(tmp_path / "nope.txt"))
    assert result.returncode == 1
    assert "no such identity file" in result.stderr


needs_both = pytest.mark.skipif(
    not (shutil.which("sops") and shutil.which("age-keygen")), reason="needs age and sops"
)


def sealed_repo(tool, repo, tmp_path):
    root, home, _env = repo
    env = with_identity(repo, tmp_path)
    (root / "environment" / "test").mkdir(parents=True)
    (root / "environment" / "test" / "manifest").write_text("shared\n")
    (root / "shared").mkdir()
    (root / "config" / "targets.dotfile").write_text("")
    (root / "config" / "profile").write_text("test\n")
    assert secret(tool, env, "init").returncode == 0
    assert secret(tool, env, "enroll", onepassword.HOST).returncode == 0
    recovery = tmp_path / "recovery.txt"
    subprocess.run(["age-keygen", "-o", str(recovery)], check=True, capture_output=True)
    pub = subprocess.run(
        ["age-keygen", "-y", str(recovery)], check=True, capture_output=True, text=True
    ).stdout.strip()
    assert secret(tool, env, "enroll", "recovery", pub).returncode == 0
    plain = tmp_path / "plain.yaml"
    plain.write_text("hosts:\n  demo: 203.0.113.55\n")
    sealed = subprocess.run(
        ["sops", "--config", str(root / ".sops.yaml"), "-e", str(plain)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    (root / "vars.enc.yaml").write_text(sealed)
    run_git(root, "add", "-A")
    return root, home, env, recovery


def ciphertext(root):
    return (root / "vars.enc.yaml").read_text().split("data:")[1].split(",")[0]


def opens(path, root):
    return (
        subprocess.run(
            ["sops", "-d", str(root / "vars.enc.yaml")],
            env=dict(os.environ, SOPS_AGE_KEY_FILE=str(path), XDG_CONFIG_HOME="/var/empty"),
            capture_output=True,
            check=False,
        ).returncode
        == 0
    )


@needs_both
def test_revoke_gives_a_new_data_key(tool, repo, tmp_path):
    root, _home, env, recovery = sealed_repo(tool, repo, tmp_path)
    before = ciphertext(root)
    assert secret(tool, env, "revoke", "recovery").returncode == 0
    assert ciphertext(root) != before
    assert not opens(recovery, root)


@needs_both
def test_rolling_this_machine_swaps_the_identity_and_keeps_the_label(tool, repo, tmp_path):
    root, home, env, _recovery = sealed_repo(tool, repo, tmp_path)
    kept = onepassword.identity_file(env, tmp_path / "old-identity.txt")
    old_public = onepassword.field(env, "username")
    before = ciphertext(root)

    result = secret(tool, env, "roll", onepassword.HOST)
    assert result.returncode == 0, result.stderr

    current = onepassword.identity_file(env, tmp_path / "new-identity.txt")
    assert not opens(kept, root)
    assert opens(current, root)
    assert ciphertext(root) != before
    new_public = onepassword.field(env, "username")
    assert new_public != old_public
    keys = (root / "config" / "keys.dotfile").read_text()
    assert f"{onepassword.HOST}" in keys and new_public in keys and old_public not in keys
    shared = (home / ".config" / "sops" / "age" / "keys.txt").read_text()
    assert kept.read_text().strip() in shared
    assert current.read_text().strip() in shared


@needs_both
def test_rolling_another_recipient_locks_the_old_key_out(tool, repo, tmp_path):
    root, _home, env, recovery = sealed_repo(tool, repo, tmp_path)
    replacement = tmp_path / "recovery2.txt"
    subprocess.run(["age-keygen", "-o", str(replacement)], check=True, capture_output=True)
    pub = subprocess.run(
        ["age-keygen", "-y", str(replacement)], check=True, capture_output=True, text=True
    ).stdout.strip()

    assert secret(tool, env, "roll", "recovery", pub).returncode == 0
    assert not opens(recovery, root)
    assert opens(replacement, root)
    assert "recovery" in (root / "config" / "keys.dotfile").read_text()


@needs_both
def test_rekey_changes_the_data_key_and_keeps_recipients(tool, repo, tmp_path):
    root, _home, env, recovery = sealed_repo(tool, repo, tmp_path)
    before = ciphertext(root)
    keys = (root / "config" / "keys.dotfile").read_text()
    assert secret(tool, env, "rekey").returncode == 0
    assert ciphertext(root) != before
    assert opens(recovery, root)
    assert (root / "config" / "keys.dotfile").read_text() == keys


@needs_both
def test_an_unreadable_file_aborts_the_roll_before_anything_changes(tool, repo, tmp_path):
    root, home, env, _recovery = sealed_repo(tool, repo, tmp_path)
    shared = home / ".config" / "sops" / "age" / "keys.txt"
    before_identity = onepassword.field(env, "credential")
    before_shared = shared.read_bytes()
    before_keys = (root / "config" / "keys.dotfile").read_text()

    stranger = tmp_path / "stranger.txt"
    subprocess.run(["age-keygen", "-o", str(stranger)], check=True, capture_output=True)
    pub = subprocess.run(
        ["age-keygen", "-y", str(stranger)], check=True, capture_output=True, text=True
    ).stdout.strip()
    rules = tmp_path / "other.yaml"
    rules.write_text(f"creation_rules:\n  - age: {pub}\n")
    plain = tmp_path / "p.yaml"
    plain.write_text("x: y\n")
    foreign = subprocess.run(
        ["sops", "--config", str(rules), "-e", str(plain)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    (root / "foreign.enc.yaml").write_text(foreign)
    run_git(root, "add", "-A")

    result = secret(tool, env, "roll", onepassword.HOST)
    assert result.returncode == 1
    assert "cannot read" in result.stderr
    assert "restored the old key" in result.stderr
    assert onepassword.field(env, "credential") == before_identity
    assert shared.read_bytes() == before_shared
    assert (root / "config" / "keys.dotfile").read_text() == before_keys


@needs_both
def test_roll_refuses_a_label_that_is_not_this_machine(tool, repo, tmp_path):
    _root, _home, env, _recovery = sealed_repo(tool, repo, tmp_path)
    result = secret(tool, env, "roll", "recovery")
    assert result.returncode == 1
    assert "is not this machine" in result.stderr


def test_roll_refuses_an_unknown_label(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archie", KEY_A)
    result = secret(tool, env, "roll", "nosuch", KEY_B)
    assert result.returncode == 1
    assert "not enrolled" in result.stderr
