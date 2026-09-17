import shutil

import pytest
from gitrepo import run_git

needs_sops = pytest.mark.skipif(
    not (shutil.which("sops") and shutil.which("age-keygen")),
    reason="needs age and sops",
)

DECLARED = "hosts:\n  parser:\n    origin: 203.0.113.77\n    port: 2222\nopen:\n  user: someone\n"


@needs_sops
def seed(secret, writer, body=DECLARED):
    return secret("edit", "vars.enc.yaml", editor=writer(body))


@needs_sops
def test_a_template_renders_from_vars(vault, writer):
    root, home, _env, secret = vault
    (root / "shared" / "ssh").mkdir()
    with open(root / "config" / "targets.dotfile", "a") as f:
        f.write("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / ".secret").write_text("")
    (root / "config" / "targets.dotfile").write_text("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / "config.tmpl").write_text(
        "Host parser\n  HostName {{ hosts.parser.origin }}\n  Port {{ hosts.parser.port }}\n"
    )
    assert seed(secret, writer).returncode == 0
    assert secret("apply").returncode == 0
    assert (home / ".ssh" / "config").read_text() == (
        "Host parser\n  HostName 203.0.113.77\n  Port 2222\n"
    )


@needs_sops
def test_the_ciphertext_keeps_the_keys_readable(vault, writer):
    root, _home, _env, secret = vault
    assert seed(secret, writer).returncode == 0
    sealed = (root / "vars.enc.yaml").read_text()
    assert "hosts:" in sealed
    assert "203.0.113.77" not in sealed


@needs_sops
def test_an_unknown_var_blocks_and_names_itself(vault, writer):
    root, _home, _env, secret = vault
    (root / "shared" / "ssh").mkdir()
    with open(root / "config" / "targets.dotfile", "a") as f:
        f.write("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / ".secret").write_text("")
    (root / "config" / "targets.dotfile").write_text("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / "config.tmpl").write_text("X {{ hosts.nope }}\n")
    seed(secret, writer)
    result = secret("status")
    assert result.returncode == 1
    assert "unresolved" in result.stdout
    assert "hosts.nope" in result.stdout


@needs_sops
def test_a_template_is_allowed_inside_a_secret_package(vault, writer, tool):
    root, _home, env, secret = vault
    (root / "shared" / "ssh").mkdir()
    with open(root / "config" / "targets.dotfile", "a") as f:
        f.write("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / ".secret").write_text("")
    (root / "shared" / "ssh" / "config.tmpl").write_text("Host x\n")
    seed(secret, writer)
    run_git(root, "add", "-A")
    assert tool("dotfile", "secret", "scan", "--staged", env=env).returncode == 0


@needs_sops
def test_var_values_become_canaries(vault, writer, tool):
    root, _home, env, secret = vault
    seed(secret, writer)
    (root / "note.md").write_text("the box lives at 203.0.113.77\n")
    run_git(root, "add", "-A")
    result = tool("dotfile", "secret", "scan", "--staged", "--all", env=env)
    assert result.returncode == 1
    assert "hosts.parser.origin" in result.stderr
    assert "203.0.113.77" not in result.stdout + result.stderr


@needs_sops
def test_open_values_are_not_canaries(vault, writer, tool):
    root, _home, env, secret = vault
    seed(secret, writer)
    (root / "note.md").write_text("signed off by someone\n")
    run_git(root, "add", "-A")
    assert tool("dotfile", "secret", "scan", "--staged", env=env).returncode == 0


@needs_sops
def test_vars_lists_names_and_never_values(vault, writer):
    root, _home, _env, secret = vault
    (root / "shared" / "ssh").mkdir()
    with open(root / "config" / "targets.dotfile", "a") as f:
        f.write("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / ".secret").write_text("")
    (root / "config" / "targets.dotfile").write_text("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / "config.tmpl").write_text("H {{ hosts.parser.origin }}\n")
    seed(secret, writer)
    result = secret("vars")
    assert result.returncode == 0
    assert "hosts.parser.origin" in result.stdout
    assert "203.0.113.77" not in result.stdout
    assert "unused" in result.stdout


@needs_sops
def test_vars_can_list_only_the_unreferenced(vault, writer):
    _root, _home, _env, secret = vault
    seed(secret, writer)
    result = secret("vars", "--unused")
    assert "hosts.parser.origin" in result.stdout


@needs_sops
def test_a_template_without_a_key_is_sealed_not_broken(vault, writer, tool):
    root, _home, env, secret = vault
    (root / "shared" / "ssh").mkdir()
    with open(root / "config" / "targets.dotfile", "a") as f:
        f.write("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / ".secret").write_text("")
    (root / "config" / "targets.dotfile").write_text("shared/ssh = ~/.ssh\n")
    (root / "shared" / "ssh" / "config.tmpl").write_text("H {{ hosts.parser.origin }}\n")
    seed(secret, writer)
    shutil.rmtree(root / "config" / "age")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    assert "sealed" in result.stdout


@needs_sops
def test_transcript_redaction_strips_var_values(vault, writer):
    _root, _home, env, secret = vault
    seed(secret, writer)
    import os
    import subprocess
    import sys

    code = (
        "from tools.transcript.native_redaction import Redactor;"
        "print(Redactor()('box at 203.0.113.77 ok'))"
    )
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True,
        text=True,
        env=dict(os.environ, **env),
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "203.0.113.77" not in result.stdout
    assert "[redacted:private]" in result.stdout
