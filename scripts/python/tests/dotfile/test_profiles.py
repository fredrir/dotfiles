import sys

import pytest


@pytest.fixture
def profiles(tmp_path, tool):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    (root / "config").mkdir(parents=True)
    (root / "config/targets.dotfile").write_text("")
    home.mkdir()
    layout = {
        "arch-linux/kde": ["shared", "linux/common", "linux/kde"],
        "arch-linux/hyprland": ["shared", "linux/common", "linux/hyprland"],
        "arch-linux/kde-hyprland": ["shared", "linux/common", "linux/kde", "linux/hyprland"],
        "macos": ["shared", "macos"],
        "ubuntu/server": ["shared", "linux/server"],
    }
    for profile, groups in layout.items():
        directory = root / "environment" / profile
        directory.mkdir(parents=True)
        (directory / "manifest").write_text("".join(group + "\n" for group in groups))

    def run(*args):
        return tool(
            "dotfile",
            "profiles",
            *args,
            env={
                "DOTFILE_ROOT": str(root),
                "HOME": str(home),
                "XDG_CONFIG_HOME": str(home / ".config"),
            },
        )

    return sorted(layout), run


def test_profiles_lists_all_manifests_through_native_cli(profiles):
    expected, run = profiles
    result = run()
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == expected


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS profile relevance")
def test_macos_relevance_omits_linux_profiles(profiles):
    _expected, run = profiles
    result = run("--relevant")
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == ["macos"]
