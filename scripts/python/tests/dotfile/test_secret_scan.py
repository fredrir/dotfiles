import subprocess

import pytest

TOKEN = "ghp_" + "abcdefghijklmnopqrstuv1234567890"
PRIVATE_VALUE = "203.0.113.77"


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


def write(root, path, text):
    target = root / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)


def stage(root, path, text):
    write(root, path, text)
    run_git(root, "add", "-A")


def canaries(home, text):
    (home / ".config" / "dotfile" / "canaries").write_text(text)


def scan(tool, env, *args):
    return tool("dotfile", "secret", "scan", *args, env=env)


def test_clean_tree_passes(tool, repo):
    root, _home, env = repo
    stage(root, "shared/notes/plain.md", "nothing interesting here\n")
    result = scan(tool, env)
    assert result.returncode == 0
    assert "clean" in result.stdout


def test_pattern_tier_reports_without_printing_the_secret(tool, repo):
    root, _home, env = repo
    stage(root, "shared/notes/leak.md", f"export TOKEN={TOKEN}\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "github-token" in result.stderr
    assert TOKEN not in result.stdout + result.stderr


def test_a_finding_survives_the_hooks_silencing_of_stdout(tool, repo):
    root, _home, env = repo
    stage(root, "shared/notes/leak.md", f"export TOKEN={TOKEN}\n")
    result = scan(tool, env, "--staged")
    assert result.returncode == 1
    assert result.stdout == ""
    assert "1 finding" in result.stderr


def test_an_identifier_named_like_a_key_is_not_a_finding(tool, repo):
    root, _home, env = repo
    stage(root, "shared/lib/keys.py", 'AGE_KEY = re.compile(r"^age1[02-9ac-hj-np-z]{58}$")\n')
    assert scan(tool, env).returncode == 0


def test_an_age_private_key_is_a_finding_however_it_is_named(tool, repo):
    root, _home, env = repo
    identity = "AGE-SECRET-KEY-1" + "Q" * 58
    stage(root, "shared/lib/leak.py", f"HARMLESS_NAME = '{identity}'\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "age-identity" in result.stderr
    assert identity not in result.stdout + result.stderr


def test_an_age_public_key_is_not_a_finding(tool, repo):
    root, _home, env = repo
    stage(root, "config/keys.dotfile", "recipients {\n  archie = age1" + "q" * 58 + "\n}\n")
    assert scan(tool, env).returncode == 0


def test_allowlist_suppresses_a_pattern(tool, repo):
    root, _home, env = repo
    write(root, "config/scan.dotfile", "allow {\n  vendor/** github-token\n}\n")
    stage(root, "vendor/lib.js", f"const t = '{TOKEN}'\n")
    assert scan(tool, env).returncode == 0


def test_allowlist_label_does_not_cover_other_labels(tool, repo):
    root, _home, env = repo
    write(root, "config/scan.dotfile", "allow {\n  vendor/** aws-key\n}\n")
    stage(root, "vendor/lib.js", f"const t = '{TOKEN}'\n")
    assert scan(tool, env).returncode == 1


def test_canary_is_reported_by_label_only(tool, repo):
    root, home, env = repo
    canaries(home, f"parser-origin = {PRIVATE_VALUE}\n")
    stage(root, "shared/notes/infra.md", f"the box lives at {PRIVATE_VALUE}\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "parser-origin" in result.stderr
    assert PRIVATE_VALUE not in result.stdout + result.stderr


def test_canary_ignores_the_allowlist(tool, repo):
    root, home, env = repo
    canaries(home, f"parser-origin = {PRIVATE_VALUE}\n")
    write(root, "config/scan.dotfile", "allow {\n  vendor/**\n}\n")
    stage(root, "vendor/lib.js", f"host = {PRIVATE_VALUE}\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "parser-origin" in result.stderr


def test_no_canaries_skips_the_tier(tool, repo):
    root, home, env = repo
    canaries(home, f"parser-origin = {PRIVATE_VALUE}\n")
    stage(root, "shared/notes/infra.md", f"host {PRIVATE_VALUE}\n")
    assert scan(tool, env, "--no-canaries").returncode == 0


def test_short_canary_is_rejected(tool, repo):
    root, home, env = repo
    canaries(home, "tiny = abc\n")
    stage(root, "shared/notes/plain.md", "abc appears everywhere\n")
    result = scan(tool, env)
    assert result.returncode == 0
    assert "too short" in result.stdout


def test_enc_file_must_carry_sops_metadata(tool, repo):
    root, _home, env = repo
    stage(root, "shared/ssh/config.enc", "host example\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "not-encrypted" in result.stderr


def test_encrypted_file_passes_and_skips_content_tiers(tool, repo):
    root, _home, env = repo
    stage(root, "shared/ssh/config.enc", f"data: ENC[AES256_GCM,data:xx] {TOKEN}\n")
    assert scan(tool, env).returncode == 0


def test_secret_package_rejects_plaintext(tool, repo):
    root, _home, env = repo
    write(root, "shared/ssh/.secret", "")
    stage(root, "shared/ssh/config", "host example\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "plaintext" in result.stderr


def test_key_filename_outside_a_secret_package(tool, repo):
    root, _home, env = repo
    stage(root, "shared/ssh/id_ed25519", "opaque\n")
    result = scan(tool, env)
    assert result.returncode == 1
    assert "key-file" in result.stderr


def test_staged_scan_sees_staged_content(tool, repo):
    root, _home, env = repo
    stage(root, "shared/notes/plain.md", "fine\n")
    run_git(root, "commit", "-qm", "base")
    stage(root, "shared/notes/leak.md", f"token {TOKEN}\n")
    assert scan(tool, env, "--staged").returncode == 1


def test_commit_range_finds_a_value_removed_before_push(tool, repo):
    root, _home, env = repo
    stage(root, "shared/notes/plain.md", "fine\n")
    run_git(root, "commit", "-qm", "base")
    stage(root, "shared/notes/leak.md", f"token {TOKEN}\n")
    run_git(root, "commit", "-qm", "leak")
    run_git(root, "rm", "-q", "shared/notes/leak.md")
    run_git(root, "commit", "-qm", "remove")
    assert scan(tool, env).returncode == 0
    assert scan(tool, env, "--commits", "HEAD~2..HEAD").returncode == 1


def test_binary_files_are_skipped_not_decoded(tool, repo):
    root, _home, env = repo
    (root / "shared").mkdir(parents=True, exist_ok=True)
    (root / "shared" / "blob.bin").write_bytes(b"\x00\x01\x02" + TOKEN.encode())
    run_git(root, "add", "-A")
    result = scan(tool, env)
    assert result.returncode == 0
    assert "not text" in result.stdout
