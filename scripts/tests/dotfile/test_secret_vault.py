import os
import shutil
import stat
import subprocess

import pytest

pytestmark = pytest.mark.skipif(
    not (shutil.which("sops") and shutil.which("age-keygen")),
    reason="needs age and sops",
)


def run_git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True)


def mode_of(path):
    return stat.S_IMODE(os.stat(path).st_mode)


@pytest.fixture
def vault(tool, tmp_path):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    root.mkdir()
    (home / ".config" / "dotfile").mkdir(parents=True)
    (home / ".ssh").mkdir()
    (root / "environment" / "test").mkdir(parents=True)
    (root / "environment" / "test" / "manifest").write_text("shared\n")
    (root / "targets").write_text("")
    (root / "shared").mkdir()
    run_git(tmp_path, "init", "-q", str(root))
    run_git(root, "config", "user.email", "test@example.com")
    run_git(root, "config", "user.name", "test")
    (home / ".config" / "dotfile" / "profile").write_text("test\n")

    env = {
        "DOTFILE_ROOT": str(root),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
    }

    def secret(*args, editor=None):
        merged = dict(env, EDITOR=editor) if editor else env
        return tool("dotfile", "secret", *args, env=merged)

    assert secret("init").returncode == 0
    assert secret("enroll", "box").returncode == 0
    return root, home, env, secret


def add_config(home, secret, text="Host box\n  Port 2222\n"):
    live = home / ".ssh" / "config"
    live.write_text(text)
    result = secret("add", str(live), "--pkg", "ssh")
    assert result.returncode == 0, result.stderr
    return live


def test_add_encrypts_and_leaves_the_live_file(vault):
    root, home, _env, secret = vault
    live = add_config(home, secret)
    sealed = root / "shared" / "ssh" / "config.enc"
    assert sealed.is_file()
    assert "ENC[AES256_GCM" in sealed.read_text()
    assert "2222" not in sealed.read_text()
    assert live.read_text() == "Host box\n  Port 2222\n"
    assert mode_of(live) == 0o600


def test_add_marks_a_new_package_and_maps_the_directory(vault):
    root, home, _env, secret = vault
    add_config(home, secret)
    assert (root / "shared" / "ssh" / ".secret").is_file()
    assert "shared/ssh = ~/.ssh" in (root / "targets").read_text()


def test_add_refuses_to_overwrite(vault):
    _root, home, _env, secret = vault
    add_config(home, secret)
    result = secret("add", str(home / ".ssh" / "config"), "--pkg", "ssh")
    assert result.returncode == 1
    assert "destination exists" in result.stderr


def test_add_requires_a_package(vault):
    _root, home, _env, secret = vault
    (home / ".ssh" / "config").write_text("x\n")
    result = secret("add", str(home / ".ssh" / "config"))
    assert result.returncode == 1
    assert "--pkg" in result.stderr


def test_clean_then_apply_round_trips_bytes(vault):
    _root, home, _env, secret = vault
    live = add_config(home, secret)
    original = live.read_bytes()
    assert secret("clean").returncode == 0
    assert not live.exists()
    assert secret("apply").returncode == 0
    assert live.read_bytes() == original
    assert mode_of(live) == 0o600


def test_apply_secures_the_package_directory(vault):
    _root, home, _env, secret = vault
    add_config(home, secret)
    os.chmod(home / ".ssh", 0o755)
    assert secret("apply").returncode == 0
    assert mode_of(home / ".ssh") == 0o700


def test_apply_is_idempotent(vault):
    _root, home, _env, secret = vault
    add_config(home, secret)
    result = secret("apply")
    assert result.returncode == 0
    assert "current" in result.stdout


def test_a_local_edit_is_reported_and_never_overwritten(vault):
    _root, home, _env, secret = vault
    live = add_config(home, secret)
    live.write_text("Host box\n  Port 9999\n")
    status = secret("status")
    assert status.returncode == 1
    assert "drifted" in status.stdout
    apply = secret("apply")
    assert apply.returncode == 1
    assert live.read_text() == "Host box\n  Port 9999\n"


def test_clean_refuses_to_destroy_a_local_edit(vault):
    _root, home, _env, secret = vault
    live = add_config(home, secret)
    live.write_text("Host box\n  Port 9999\n")
    assert secret("clean").returncode == 1
    assert live.exists()


def test_force_discards_a_local_edit(vault):
    _root, home, _env, secret = vault
    live = add_config(home, secret)
    live.write_text("Host box\n  Port 9999\n")
    assert secret("apply", "--force").returncode == 0
    assert live.read_text() == "Host box\n  Port 2222\n"


def test_dry_run_changes_nothing(vault):
    _root, home, _env, secret = vault
    live = add_config(home, secret)
    secret("clean")
    result = secret("apply", "-n")
    assert result.returncode == 0
    assert "would apply" in result.stdout
    assert not live.exists()


def test_a_machine_without_a_key_still_links(vault, tool):
    _root, home, env, secret = vault
    live = add_config(home, secret)
    secret("clean")
    shutil.rmtree(home / ".config" / "dotfile" / "age")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert "sealed" in result.stdout
    assert not live.exists()


def test_link_materialises_and_never_symlinks_a_secret(vault, tool):
    _root, home, env, secret = vault
    live = add_config(home, secret)
    secret("clean")
    assert tool("dotfile", "link", "test", env=env).returncode == 0
    assert live.is_file()
    assert not live.is_symlink()


def test_plaintext_inside_a_secret_package_blocks_apply(vault):
    root, home, _env, secret = vault
    add_config(home, secret)
    (root / "shared" / "ssh" / "notes.txt").write_text("oops\n")
    result = secret("apply")
    assert result.returncode == 1
    assert "plaintext" in result.stdout


def test_binary_content_survives_the_round_trip(vault):
    _root, home, _env, secret = vault
    live = home / ".ssh" / "id_ed25519"
    payload = bytes(range(256)) * 4
    live.write_bytes(payload)
    assert secret("add", str(live), "--pkg", "ssh").returncode == 0
    assert secret("clean").returncode == 0
    assert secret("apply").returncode == 0
    assert live.read_bytes() == payload


def test_scan_accepts_the_secret_package(vault, tool):
    root, home, env, secret = vault
    add_config(home, secret)
    run_git(root, "add", "-A")
    assert tool("dotfile", "secret", "scan", "--staged", env=env).returncode == 0


def test_edit_reencrypts_and_reapplies(vault):
    root, home, _env, secret = vault
    live = add_config(home, secret)
    editor = root.parent / "editor.sh"
    editor.write_text('#!/usr/bin/env bash\nprintf "Host box\\n  Port 4242\\n" > "$1"\n')
    editor.chmod(0o755)
    result = secret("edit", "shared/ssh/config.enc", editor=str(editor))
    assert result.returncode == 0, result.stderr
    assert live.read_text() == "Host box\n  Port 4242\n"
    sealed = (root / "shared" / "ssh" / "config.enc").read_text()
    assert "4242" not in sealed


def test_edit_resolves_a_destination_path(vault):
    root, home, _env, secret = vault
    live = add_config(home, secret)
    editor = root.parent / "editor.sh"
    editor.write_text('#!/usr/bin/env bash\nprintf "Host box\\n  Port 7000\\n" > "$1"\n')
    editor.chmod(0o755)
    result = secret("edit", str(live), editor=str(editor))
    assert result.returncode == 0, result.stderr
    assert live.read_text() == "Host box\n  Port 7000\n"


def test_edit_without_changes_is_not_an_error(vault):
    _root, home, _env, secret = vault
    add_config(home, secret)
    result = secret("edit", "shared/ssh/config.enc", editor="/bin/true")
    assert result.returncode == 0
    assert "unchanged" in result.stdout


def test_edit_rejects_an_unknown_path(vault):
    _root, home, _env, secret = vault
    add_config(home, secret)
    result = secret("edit", "shared/nope/missing.enc")
    assert result.returncode == 1
    assert "no tracked secret" in result.stderr
