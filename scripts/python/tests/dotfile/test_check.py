import os

import pytest


@pytest.fixture
def ctx(tmp_path, tool):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    for path in (
        root / "shared",
        root / "config",
        root / "environment/test",
        home / ".config/dotfile",
    ):
        path.mkdir(parents=True)
    (root / "config/targets.dotfile").write_text("")
    (root / "environment/test/manifest").write_text("shared\n")
    (root / "config" / "profile").write_text("test\n")
    env = {
        "DOTFILE_ROOT": str(root),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local/share"),
        "SHELL": "/bin/zsh",
        "USER": "",
        "HWTUNE_HOST": "fixture",
        "HWTUNE_BENCHMARKS": str(tmp_path / "benchmarks"),
    }

    def doctor(*arguments, **overrides):
        return tool("dotfile", "doctor", *arguments, env=dict(env, **overrides))

    return root, home, doctor


def write_requires(ctx, text):
    (ctx[0] / "config/requirements.dotfile").write_text(text)


def write_project(ctx, *commands):
    scripts = ctx[0] / "scripts/python"
    scripts.mkdir(parents=True, exist_ok=True)
    entries = "".join(f'{name} = "tools.commands:{name}"\n' for name in commands)
    (scripts / "pyproject.toml").write_text(
        f'[project]\nname = "tools"\nversion = "0.1.0"\n[project.scripts]\n{entries}'
    )


def install_commands(ctx, *commands):
    bindir = ctx[1] / "dotfiles/.bin"
    tool_bindir = ctx[1] / ".local/share/uv/tools/tools/bin"
    bindir.mkdir(parents=True, exist_ok=True)
    tool_bindir.mkdir(parents=True, exist_ok=True)
    for name in commands:
        source = tool_bindir / name
        source.write_text("#!/bin/sh\nexit 0\n")
        source.chmod(0o755)
        (bindir / name).symlink_to(source)
    return str(bindir)


def test_reads_every_entry_kind(ctx):
    write_requires(
        ctx,
        "# a comment\nshared {\n native-test-missing = install-me\n ?native-test-optional # trailing\n font Nonexistent Test Font\n file ~/.config/missing-file\n}\n",
    )
    result = ctx[2]("--all")
    assert result.returncode == 1
    for expected in ("tools", "fonts", "files", "optional", "install-me", "missing-file"):
        assert expected in result.stdout


def test_missing_file_means_no_requirements(ctx):
    result = ctx[2]()
    assert result.returncode == 0, result.stderr
    assert "tools" not in result.stdout


def test_rejects_a_group_that_is_not_in_the_repository(ctx):
    write_requires(ctx, "linux/kde {\n konsole\n}\n")
    result = ctx[2]()
    assert result.returncode == 1
    assert "unknown group: linux/kde" in result.stderr


def test_rejects_an_entry_outside_a_group(ctx):
    write_requires(ctx, "git\n")
    result = ctx[2]()
    assert result.returncode == 1
    assert "outside a block" in result.stderr


def test_reads_declared_project_commands(ctx):
    write_project(ctx, "size", "count")
    result = ctx[2]("--all")
    assert result.returncode == 1
    assert "size" in result.stdout and "count" in result.stdout


def test_accepts_commands_installed_as_uv_tools(ctx):
    write_project(ctx, "count", "size")
    bindir = install_commands(ctx, "count", "size")
    result = ctx[2](PATH=bindir + os.pathsep + "/usr/bin")
    assert result.returncode == 0, result.stdout
    assert "workstation commands need attention" not in result.stdout


def test_reports_a_missing_public_command(ctx):
    write_project(ctx, "count", "size")
    bindir = install_commands(ctx, "count")
    result = ctx[2](PATH=bindir + os.pathsep + "/usr/bin")
    assert result.returncode == 1
    assert "size" in result.stdout and "missing from ~/dotfiles/.bin" in result.stdout


def test_rejects_a_public_command_outside_the_uv_tool_directory(ctx):
    write_project(ctx, "count")
    bindir = ctx[1] / "dotfiles/.bin"
    bindir.mkdir(parents=True)
    command = bindir / "count"
    command.write_text("#!/bin/sh\nexit 0\n")
    command.chmod(0o755)
    result = ctx[2](PATH=str(bindir) + os.pathsep + "/usr/bin")
    assert result.returncode == 1
    assert "not installed by uv" in result.stdout


def test_reports_a_uv_command_shadowed_earlier_on_path(ctx):
    write_project(ctx, "count")
    bindir = install_commands(ctx, "count")
    earlier = ctx[1] / "earlier"
    earlier.mkdir()
    command = earlier / "count"
    command.write_text("#!/bin/sh\nexit 0\n")
    command.chmod(0o755)
    result = ctx[2](PATH=str(earlier) + os.pathsep + bindir + os.pathsep + "/usr/bin")
    assert result.returncode == 1
    assert "shadowed on PATH" in result.stdout


def test_brewfile_lists_formulae_and_casks(ctx):
    root, home, doctor = ctx
    (root / "macos").mkdir()
    (root / "environment/test/manifest").write_text("shared\nmacos\n")
    (root / "macos/Brewfile").write_text(
        '# comment\ntap "homebrew/bundle"\nbrew "starship"\nbrew \'eza\'\nbrew "some/tap/tool"\ncask "kitty"\n'
    )
    binary = home / "bin"
    binary.mkdir()
    script = binary / "brew"
    script.write_text("#!/bin/sh\nprintf 'starship\\neza\\ntool\\nkitty\\n'\n")
    script.chmod(0o755)
    result = doctor(PATH=str(binary) + os.pathsep + "/usr/bin")
    assert result.returncode == 0, result.stdout
    assert "4 installed" in result.stdout


def test_pkglist_drops_comments_and_blank_lines(ctx):
    root, home, doctor = ctx
    (root / "environment/test/pkglist.txt").write_text("git\n\n# comment\nneovim\n")
    binary = home / "bin"
    binary.mkdir()
    script = binary / "pacman"
    script.write_text("#!/bin/sh\nprintf 'git\\nneovim\\n'\n")
    script.chmod(0o755)
    result = doctor(PATH=str(binary) + os.pathsep + "/usr/bin")
    assert result.returncode == 0, result.stdout
    assert "2 installed" in result.stdout


def test_missing_tools_show_package_hints_below_the_section(ctx):
    write_requires(ctx, "shared {\n missing-tool-one\n missing-tool-two = install-package\n}\n")
    result = ctx[2]()
    assert result.returncode == 1
    assert result.stdout.index("tools") < result.stdout.index("missing-tool-one")
    assert "install-package" in result.stdout


def test_clips_items_and_all_lists_every_finding(ctx):
    write_requires(
        ctx, "shared {\n" + "".join(f" missing-native-{index:02}\n" for index in range(15)) + "}\n"
    )
    result = ctx[2]()
    assert "and 3 more" in result.stdout
    assert "missing-native-14" not in result.stdout
    full = ctx[2]("--all")
    assert "missing-native-14" in full.stdout
    assert "and 3 more" not in full.stdout


def test_a_requirement_in_two_groups_is_checked_once(ctx):
    root, _home, doctor = ctx
    (root / "macos").mkdir()
    (root / "environment/test/manifest").write_text("shared\nmacos\n")
    write_requires(ctx, "shared {\n ?missing-native-check\n}\nmacos {\n missing-native-check\n}\n")
    result = doctor()
    assert result.returncode == 1
    assert result.stdout.count("missing-native-check") == 1
    assert "1 missing" in result.stdout
    assert "optional" not in result.stdout
