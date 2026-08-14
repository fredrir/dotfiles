import pytest

from tools.dotfile import profiles


@pytest.fixture
def envdir(tmp_path):
    layout = {
        "arch-linux/kde": ["shared", "linux/common", "linux/kde"],
        "arch-linux/hyprland": ["shared", "linux/common", "linux/hyprland"],
        "arch-linux/kde-hyprland": ["shared", "linux/common", "linux/kde", "linux/hyprland"],
        "macos": ["shared", "macos"],
        "ubuntu/server": ["shared", "linux/server"],
    }
    for profile, groups in layout.items():
        directory = tmp_path / profile
        directory.mkdir(parents=True)
        (directory / "manifest").write_text("".join(group + "\n" for group in groups))
    return str(tmp_path)


def test_filters_for_arch_with_kde_only(envdir):
    assert profiles.filter_profiles(envdir, "arch-linux", ["kde"]) == ["arch-linux/kde"]


def test_includes_combined_profile_when_both_desktops_are_installed(envdir):
    assert profiles.filter_profiles(envdir, "arch-linux", ["kde", "hyprland"]) == [
        "arch-linux/hyprland",
        "arch-linux/kde",
        "arch-linux/kde-hyprland",
    ]


def test_filters_profiles_by_operating_system(envdir):
    assert profiles.filter_profiles(envdir, "macos", []) == ["macos"]
    assert profiles.filter_profiles(envdir, "ubuntu", []) == ["ubuntu/server"]


def test_normalizes_explicit_environment_override():
    assert profiles.normalize_profile_arg("--arch-linux/hyprland") == "arch-linux/hyprland"
    assert profiles.normalize_profile_arg("arch-linux/kde") == "arch-linux/kde"
    assert profiles.normalize_profile_arg("--") == "--"


def test_detects_platform_from_os_release():
    assert profiles.detect_linux_platform({"ID": "arch"}) == "arch-linux"
    assert profiles.detect_linux_platform({"ID": "ubuntu"}) == "ubuntu"
    assert profiles.detect_linux_platform({"ID": "cachyos", "ID_LIKE": "arch"}) == "arch-linux"
    assert profiles.detect_linux_platform({"ID": "neon", "ID_LIKE": "ubuntu debian"}) == "ubuntu"
    assert profiles.detect_linux_platform({"ID": "gentoo"}) == ""
