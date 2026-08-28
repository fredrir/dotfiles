import os
import shutil

KDE_COMMANDS = ("plasmashell", "startplasma-wayland", "startplasma-x11")
HYPRLAND_COMMANDS = ("Hyprland", "hyprctl")

DESKTOP_GROUPS = {
    "linux/kde": "kde",
    "linux/hyprland": "hyprland",
}


def list_profiles(envdir):
    found = []
    for parent, _dirnames, filenames in os.walk(envdir):
        if "manifest" in filenames:
            found.append(os.path.relpath(parent, envdir))
    return sorted(found)


def read_os_release(path="/etc/os-release"):
    values = {}
    try:
        with open(path, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except OSError:
        return values
    for line in lines:
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        value = value.strip('"').strip("'")
        values[key] = value
    return values


def detect_linux_platform(os_release=None):
    values = read_os_release() if os_release is None else os_release
    platform_id = values.get("ID", "")
    id_like = values.get("ID_LIKE", "")
    if platform_id == "arch":
        return "arch-linux"
    if platform_id == "ubuntu":
        return "ubuntu"
    likes = id_like.split()
    if "arch" in likes:
        return "arch-linux"
    if "ubuntu" in likes:
        return "ubuntu"
    return ""


def detect_platform():
    system = os.uname().sysname
    if system == "Darwin":
        return "macos"
    if system == "Linux":
        return detect_linux_platform()
    return ""


def detect_installed_desktops():
    desktops = []
    if any(shutil.which(command) for command in KDE_COMMANDS):
        desktops.append("kde")
    if any(shutil.which(command) for command in HYPRLAND_COMMANDS):
        desktops.append("hyprland")
    return desktops


def manifest_group_lines(manifest):
    groups = []
    with open(manifest, encoding="utf-8") as handle:
        for line in handle.read().splitlines():
            group = line.split("#", 1)[0].strip()
            if group:
                groups.append(group)
    return groups


def profile_matches_host(envdir, profile, platform, desktops):
    if platform == "macos":
        if profile != "macos":
            return False
    elif profile.split("/")[0] != platform:
        return False
    manifest = os.path.join(envdir, profile, "manifest")
    for group in manifest_group_lines(manifest):
        required = DESKTOP_GROUPS.get(group)
        if required and required not in desktops:
            return False
    return True


def filter_profiles(envdir, platform, desktops):
    return [
        profile
        for profile in list_profiles(envdir)
        if profile_matches_host(envdir, profile, platform, desktops)
    ]


def list_relevant_profiles(envdir):
    platform = detect_platform()
    if not platform:
        return []
    return filter_profiles(envdir, platform, detect_installed_desktops())


def normalize_profile_arg(value):
    if value.startswith("--") and len(value) > 2:
        return value[2:]
    return value
