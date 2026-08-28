import os
import shutil
import stat
from types import SimpleNamespace

import pytest
from gitrepo import run_git

from tools.dotfile.secret.vault import PLAIN, SEALED, TMPL
from tools.dotfile.system import UNREADABLE, installed, mode_for, needs_install

needs_sops = pytest.mark.skipif(
    not (shutil.which("sops") and shutil.which("age-keygen")),
    reason="needs age and sops",
)


@pytest.fixture
def systemd(tool, tmp_path):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    fake = tmp_path / "etc"
    root.mkdir()
    (root / "config").mkdir()
    fake.mkdir()
    (home / ".config" / "dotfile").mkdir(parents=True)
    (root / "environment" / "test").mkdir(parents=True)
    (root / "environment" / "test" / "manifest").write_text("shared\n")
    (root / "shared").mkdir()
    run_git(tmp_path, "init", "-q", str(root))
    run_git(root, "config", "user.email", "test@example.com")
    run_git(root, "config", "user.name", "test")
    (home / ".config" / "dotfile" / "profile").write_text("test\n")

    pkg = root / "shared" / "netgear"
    pkg.mkdir(parents=True)
    (pkg / ".system").write_text("")
    (root / "config" / "targets.dotfile").write_text(f"shared/netgear/etc = {fake}\n")

    env = {
        "DOTFILE_ROOT": str(root),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
    }

    def system(*args):
        return tool("dotfile", "system", *args, env=env)

    return root, fake, pkg, env, system


def place(pkg, rel, text):
    target = pkg / "etc" / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)
    return target


def test_absent_when_nothing_is_installed(systemd):
    _root, _fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    result = system("status")
    assert result.returncode == 0
    assert "absent" in result.stdout


def test_current_when_the_destination_matches(systemd):
    _root, fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (fake / "widget.conf").write_text("one\n")
    result = system("status")
    assert result.returncode == 0
    assert "current" in result.stdout


def test_drifted_when_the_destination_differs(systemd):
    _root, fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (fake / "widget.conf").write_text("two\n")
    result = system("status")
    assert result.returncode == 1
    assert "drifted" in result.stdout


def test_drifted_when_the_destination_mode_differs(systemd):
    _root, fake, pkg, _env, system = systemd
    source = place(pkg, "widget.conf", "one\n")
    source.chmod(0o755)
    (fake / "widget.conf").write_text("one\n")
    result = system("status")
    assert result.returncode == 1
    assert "mode 0644, want 0755" in result.stdout


def test_rendered_templates_are_private(tmp_path):
    source = tmp_path / "secret.conf.tmpl"
    source.write_text("password={{ password }}\n")
    entry = SimpleNamespace(kind=TMPL, src=str(source))
    assert mode_for(entry) == 0o600


def test_root_owned_private_file_is_sealed_when_permission_denied(tmp_path, monkeypatch):
    target = tmp_path / "secret.conf"
    target.write_text("private\n")
    entry = SimpleNamespace(kind=TMPL, dst=str(target))
    metadata = SimpleNamespace(st_mode=stat.S_IFREG | 0o600, st_uid=0, st_gid=0)
    monkeypatch.setattr(os, "stat", lambda _path: metadata)

    def permission_denied(*_args, **_kwargs):
        raise PermissionError

    monkeypatch.setattr("builtins.open", permission_denied)
    assert installed(entry) == (None, SEALED)
    assert needs_install(SEALED)


def test_permission_denied_plain_file_remains_unreadable(tmp_path, monkeypatch):
    target = tmp_path / "plain.conf"
    target.write_text("plain\n")
    entry = SimpleNamespace(kind=PLAIN, dst=str(target))

    def permission_denied(*_args, **_kwargs):
        raise PermissionError

    monkeypatch.setattr("builtins.open", permission_denied)
    assert installed(entry) == (None, UNREADABLE)


def test_diff_shows_the_change(systemd):
    _root, fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (fake / "widget.conf").write_text("two\n")
    result = system("diff")
    assert "-two" in result.stdout
    assert "+one" in result.stdout


def test_diff_is_quiet_when_nothing_differs(systemd):
    _root, fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (fake / "widget.conf").write_text("one\n")
    assert "nothing to install" in system("diff").stdout


def test_dry_run_names_the_command_and_writes_nothing(systemd):
    _root, fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    result = system("install", "-n")
    assert result.returncode == 0
    assert "sudo install" in result.stdout
    assert "-o root -g root" in result.stdout
    assert not (fake / "widget.conf").exists()


def test_a_destination_under_home_is_refused(tool, tmp_path, systemd):
    root, _fake, pkg, env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (root / "config" / "targets.dotfile").write_text(
        f"shared/netgear/etc = {env['HOME']}/somewhere\n"
    )
    result = system("status")
    assert result.returncode == 1
    assert "refused" in result.stdout
    assert "under $HOME" in result.stdout


def test_a_relative_destination_is_refused(systemd):
    root, _fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (root / "config" / "targets.dotfile").write_text("shared/netgear/etc = not/absolute\n")
    result = system("status")
    assert result.returncode == 1
    assert "refused" in result.stdout


def test_install_refuses_to_run_while_anything_is_unresolved(systemd):
    _root, _fake, pkg, _env, system = systemd
    place(pkg, "widget.conf.tmpl", "value = {{ nope.missing }}\n")
    result = system("install", "-n")
    assert result.returncode == 1
    assert "unresolved" in result.stdout


def test_a_plain_file_is_not_a_plaintext_violation(systemd):
    _root, fake, pkg, _env, system = systemd
    place(pkg, "widget.conf", "one\n")
    (fake / "widget.conf").write_text("one\n")
    assert "plaintext" not in system("status").stdout


def test_add_mirrors_the_destination_tree(tool, tmp_path, systemd):
    root, fake, _pkg, env, _system = systemd
    source = fake / "deep" / "thing.conf"
    source.parent.mkdir()
    source.write_text("hello\n")
    result = tool(
        "dotfile", "system", "add", str(source), "--pkg", "adopted", "--group", "shared", env=env
    )
    assert result.returncode == 0, result.stderr
    mirrored = root / "shared" / "adopted" / str(source).lstrip("/")
    assert mirrored.read_text() == "hello\n"
    assert (root / "shared" / "adopted" / ".system").is_file()


def test_add_refuses_a_source_under_home(tool, systemd):
    _root, _fake, _pkg, env, _system = systemd
    victim = os.path.join(env["HOME"], "mine.conf")
    with open(victim, "w", encoding="utf-8") as handle:
        handle.write("x\n")
    result = tool(
        "dotfile", "system", "add", victim, "--pkg", "adopted", "--group", "shared", env=env
    )
    assert result.returncode == 1
    assert "under $HOME" in result.stderr


@needs_sops
def test_a_template_renders_from_vars(tool, systemd, writer):
    _root, fake, pkg, env, system = systemd
    assert tool("dotfile", "secret", "init", env=env).returncode == 0
    assert tool("dotfile", "secret", "enroll", "box", env=env).returncode == 0
    seed = dict(env, EDITOR=writer("net:\n  mac: aa:bb:cc:dd:ee:ff\n"))
    assert tool("dotfile", "secret", "edit", "vars.enc.yaml", env=seed).returncode == 0
    place(pkg, "link.conf.tmpl", "MACAddress={{ net.mac }}\n")
    (fake / "link.conf").write_text("MACAddress=aa:bb:cc:dd:ee:ff\n")
    (fake / "link.conf").chmod(0o600)
    result = system("status")
    assert result.returncode == 0, result.stdout
    assert "current" in result.stdout

    (fake / "link.conf").write_text("MACAddress=private-old-value\n")
    (fake / "link.conf").chmod(0o600)
    result = system("diff")
    assert result.returncode == 0
    assert "private rendered content differs" in result.stdout
    assert "aa:bb:cc:dd:ee:ff" not in result.stdout
    assert "private-old-value" not in result.stdout
