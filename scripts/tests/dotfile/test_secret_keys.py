import shutil
import subprocess

import pytest
import typer

from tools.dotfile.secret import doctor, keys

KEY_A = "age1" + "q" * 58
KEY_B = "age1" + "p" * 58


class Ctx:
    def __init__(self, root):
        self.root = str(root)


def run_git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True)


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    root.mkdir()
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


def write_keys(root, text):
    (root / "keys.dotfile").write_text(text)


def secret(tool, env, *args):
    return tool("dotfile", "secret", *args, env=env)


def test_parses_a_recipients_block(repo):
    root, _home, _env = repo
    write_keys(root, f"recipients {{\n  archpc = {KEY_A}\n  recovery = {KEY_B}\n}}\n")
    assert keys.load_recipients(Ctx(root)) == {"archpc": KEY_A, "recovery": KEY_B}


def test_missing_file_is_empty(repo):
    root, _home, _env = repo
    assert keys.load_recipients(Ctx(root)) == {}


def test_rejects_a_value_that_is_not_an_age_key(repo):
    root, _home, _env = repo
    write_keys(root, "recipients {\n  archpc = not-a-key\n}\n")
    with pytest.raises(typer.Exit):
        keys.load_recipients(Ctx(root))


def test_rejects_a_duplicate_label(repo):
    root, _home, _env = repo
    write_keys(root, f"recipients {{\n  archpc = {KEY_A}\n  archpc = {KEY_B}\n}}\n")
    with pytest.raises(typer.Exit):
        keys.load_recipients(Ctx(root))


def test_rejects_an_unterminated_block(repo):
    root, _home, _env = repo
    write_keys(root, f"recipients {{\n  archpc = {KEY_A}\n")
    with pytest.raises(typer.Exit):
        keys.load_recipients(Ctx(root))


def test_documents_are_sorted_and_stable():
    recipients = {"zeta": KEY_B, "alpha": KEY_A}
    assert keys.keys_document(recipients).index("alpha") < keys.keys_document(recipients).index(
        "zeta"
    )
    assert keys.sops_document(recipients) == f"creation_rules:\n  - age: {KEY_A},{KEY_B}\n"


def test_empty_recipients_produce_no_sops_document():
    assert keys.sops_document({}) == ""


def test_enroll_writes_both_files(tool, repo):
    root, _home, env = repo
    assert secret(tool, env, "enroll", "archpc", KEY_A).returncode == 0
    assert (root / "keys.dotfile").read_text() == f"recipients {{\n  archpc = {KEY_A}\n}}\n"
    assert KEY_A in (root / ".sops.yaml").read_text()


def test_enroll_is_idempotent(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    result = secret(tool, env, "enroll", "archpc", KEY_A)
    assert result.returncode == 0
    assert "already enrolled" in result.stdout


def test_enroll_refuses_a_key_held_under_another_label(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    result = secret(tool, env, "enroll", "laptop", KEY_A)
    assert result.returncode == 1
    assert "already enrolled as 'archpc'" in result.stderr


def test_enroll_refuses_to_replace_a_label(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    result = secret(tool, env, "enroll", "archpc", KEY_B)
    assert result.returncode == 1
    assert "revoke it first" in result.stderr


def test_revoke_removes_and_regenerates(tool, repo):
    root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    secret(tool, env, "enroll", "recovery", KEY_B)
    assert secret(tool, env, "revoke", "archpc").returncode == 0
    assert (root / "keys.dotfile").read_text() == f"recipients {{\n  recovery = {KEY_B}\n}}\n"
    assert KEY_A not in (root / ".sops.yaml").read_text()


def test_revoke_refuses_the_last_recipient(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    result = secret(tool, env, "revoke", "archpc")
    assert result.returncode == 1
    assert "only recipient" in result.stderr


def test_revoke_warns_that_rewrapping_is_not_rotation(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    secret(tool, env, "enroll", "recovery", KEY_B)
    result = secret(tool, env, "revoke", "archpc")
    assert "rotate the secrets themselves" in result.stdout


def test_sync_rewrites_a_drifted_sops_file(tool, repo):
    root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    (root / ".sops.yaml").write_text("creation_rules: []\n")
    assert keys.sops_drifted(Ctx(root), {"archpc": KEY_A})
    assert secret(tool, env, "sync").returncode == 0
    assert not keys.sops_drifted(Ctx(root), {"archpc": KEY_A})


def test_sync_without_recipients_fails(tool, repo):
    _root, _home, env = repo
    result = secret(tool, env, "sync")
    assert result.returncode == 1
    assert "no recipients" in result.stderr


def test_keys_lists_what_is_enrolled(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    result = secret(tool, env, "keys")
    assert "archpc" in result.stdout
    assert KEY_A in result.stdout


def test_doctor_fails_on_a_fresh_repository(tool, repo):
    _root, _home, env = repo
    result = secret(tool, env, "doctor")
    assert result.returncode == 1
    assert "identity" in result.stdout
    assert "recipients" in result.stdout


def test_doctor_reports_a_missing_recovery_key(tool, repo):
    _root, _home, env = repo
    secret(tool, env, "enroll", "archpc", KEY_A)
    result = secret(tool, env, "doctor")
    assert "no recovery key" in result.stdout


@pytest.mark.skipif(not shutil.which("age-keygen"), reason="needs age")
def test_doctor_tells_a_duplicate_identity_from_a_different_one(repo, tmp_path):
    root, home, _env = repo
    ctx = Ctx(root)
    ctx.state_dir = str(home / ".config" / "dotfile")
    ctx.home = str(home)

    identity = home / ".config" / "dotfile" / "age" / "keys.txt"
    identity.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["age-keygen", "-o", str(identity)], check=True, capture_output=True)
    stray = home / ".config" / "sops" / "age" / "keys.txt"
    stray.parent.mkdir(parents=True)

    assert doctor.strays_row(ctx)[0] == "ok"

    shutil.copy(identity, stray)
    assert "same key" in doctor.strays_row(ctx)[3][0][1]

    stray.unlink()
    subprocess.run(["age-keygen", "-o", str(stray)], check=True, capture_output=True)
    assert "a different key" in doctor.strays_row(ctx)[3][0][1]

    stray.write_text("junk\n")
    assert "not readable" in doctor.strays_row(ctx)[3][0][1]
