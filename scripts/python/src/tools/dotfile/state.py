import os
import sys

import typer

from tools.core.paths import dotfiles_root, home, tilde


class Context:
    def __init__(self):
        self.root = str(dotfiles_root())
        self.home = str(home())
        config_home = os.environ.get("XDG_CONFIG_HOME") or os.path.join(self.home, ".config")
        self.state_dir = os.path.join(config_home, "dotfile")
        self.targets_file = os.path.join(self.root, "config/targets.dotfile")
        self.packages_config = os.path.join(self.root, "config/packages.dotfile")
        self.requires_file = os.path.join(self.root, "config/requirements.dotfile")
        self.pins_file = os.path.join(self.root, "config/pins.dotfile")
        self.packages_doc = os.path.join(self.root, "PACKAGES.md")
        self.overrides_file = os.path.join(self.state_dir, "overrides")
        self.environment_dir = os.path.join(self.root, "environment")
        self.dry = False
        self.targets = {}
        self.overrides = {}
        self.link_groups = []
        self.active_override_dirs = []
        self.conflicts = []
        self.package_descriptions = {}
        self.merge_entries = []
        self.merge_paths = set()


def log(message):
    print(message)


def die(message):
    print(f"dotfile: {message}", file=sys.stderr)
    raise typer.Exit(1)


def run_op(ctx, words, action):
    if ctx.dry:
        log("  would: " + " ".join(words))
    else:
        action()


def shorten(ctx, path):
    return tilde(path)


def trim(value):
    return value.strip(" \t\n\r\f\v")


def canon(path):
    out = ""
    for segment in path.split("/"):
        if segment in ("", "."):
            continue
        if segment == "..":
            out = out.rpartition("/")[0]
        else:
            out = f"{out}/{segment}"
    return out or "/"


def resolve_link(link):
    target = os.readlink(link)
    if target.startswith("/"):
        return canon(target)
    return canon(os.path.dirname(link) + "/" + target)


def resolve_path(path):
    directory = os.path.dirname(path)
    base = os.path.basename(path)
    if not os.path.isdir(directory):
        return path
    resolved = os.path.join(os.path.realpath(directory), base)
    if os.path.islink(resolved):
        return resolve_link(resolved)
    return resolved


def owned_by_repo(ctx, path):
    return path.startswith(ctx.root + "/")


def sorted_entries(directory):
    try:
        return sorted(os.listdir(directory))
    except OSError:
        return []


def saved_profile(ctx):
    profile_file = os.path.join(ctx.state_dir, "profile")
    if os.path.isfile(profile_file):
        with open(profile_file, encoding="utf-8") as handle:
            return handle.read().rstrip("\n")
    return ""


def resolve_profile(ctx, profile):
    return profile or saved_profile(ctx)


def save_profile(ctx, profile):
    if ctx.dry:
        return
    os.makedirs(ctx.state_dir, exist_ok=True)
    with open(os.path.join(ctx.state_dir, "profile"), "w", encoding="utf-8") as handle:
        handle.write(profile + "\n")


def list_profiles(ctx):
    found = []
    for parent, _dirnames, filenames in os.walk(ctx.environment_dir):
        if "manifest" in filenames:
            found.append(os.path.relpath(parent, ctx.environment_dir))
    return sorted(found)


def require_manifest(ctx, profile):
    manifest = os.path.join(ctx.environment_dir, profile, "manifest") if profile else ""
    if profile and os.path.isfile(manifest):
        return manifest
    if not profile:
        print("dotfile: no profile selected (run ./setup.sh or pass one)", file=sys.stderr)
    else:
        print(f"dotfile: no manifest for profile '{profile}'", file=sys.stderr)
    print("available profiles:", file=sys.stderr)
    for name in list_profiles(ctx):
        print(f"  {name}", file=sys.stderr)
    raise typer.Exit(1)


def manifest_groups(manifest):
    groups = []
    with open(manifest, encoding="utf-8") as handle:
        for line in handle.read().splitlines():
            group = trim(line.split("#", 1)[0])
            if group:
                groups.append(group)
    return groups


def load_overrides(ctx):
    ctx.overrides = {}
    if not os.path.isfile(ctx.overrides_file):
        return
    with open(ctx.overrides_file, encoding="utf-8") as handle:
        for line in handle.read().splitlines():
            if "=" not in line:
                continue
            group, _, name = line.partition("=")
            ctx.overrides[group] = name


def save_overrides(ctx):
    if ctx.dry:
        return
    os.makedirs(ctx.state_dir, exist_ok=True)
    lines = [f"{group}={name}" for group, name in sorted(ctx.overrides.items())]
    with open(ctx.overrides_file, "w", encoding="utf-8") as handle:
        handle.write("".join(line + "\n" for line in lines))


def available_overrides(ctx, group):
    overrides_dir = os.path.join(ctx.root, group, "overrides")
    names = [
        name
        for name in sorted_entries(overrides_dir)
        if os.path.isdir(os.path.join(overrides_dir, name))
    ]
    return " ".join(names)


def select_override(ctx, group, name):
    overrides_dir = os.path.join(ctx.root, group, "overrides")
    if not os.path.isdir(overrides_dir):
        die(f"group has no overrides: {group}")
    if name != "none" and not os.path.isdir(os.path.join(overrides_dir, name)):
        die(f"unknown override '{name}' for {group} (available: {available_overrides(ctx, group)})")
    ctx.overrides[group] = name


def pending_overrides(ctx, manifest):
    return [
        group
        for group in manifest_groups(manifest)
        if os.path.isdir(os.path.join(ctx.root, group, "overrides"))
        and not ctx.overrides.get(group, "")
    ]


def collect_groups(ctx, manifest, notes=True):
    ctx.link_groups = []
    ctx.active_override_dirs = []
    for group in manifest_groups(manifest):
        ctx.link_groups.append(group)
        overrides_dir = os.path.join(ctx.root, group, "overrides")
        if not os.path.isdir(overrides_dir):
            continue
        name = ctx.overrides.get(group, "")
        if not name:
            if notes:
                log(
                    f"  note: '{group}' has machine overrides ({available_overrides(ctx, group)}), none selected"
                )
                log(f"        select one: dotfile link --override {group}=<name>  (or =none)")
            continue
        if name == "none":
            continue
        override_dir = os.path.join(overrides_dir, name)
        if os.path.isdir(override_dir):
            ctx.link_groups.append(f"{group}/overrides/{name}")
            ctx.active_override_dirs.append(override_dir)
        elif notes:
            log(f"  skip missing override: {group}/overrides/{name}")


def each_package(ctx):
    for group in ctx.link_groups:
        directory = os.path.join(ctx.root, group)
        if not os.path.isdir(directory):
            yield ("no-group", "", group)
            continue
        for name in sorted_entries(directory):
            pkgdir = os.path.join(directory, name)
            if not os.path.isdir(pkgdir):
                continue
            if name == "overrides":
                continue
            if os.path.exists(os.path.join(pkgdir, ".nolink")):
                yield ("nolink", pkgdir, f"{group}/{name}")
            elif os.path.exists(os.path.join(pkgdir, ".secret")):
                yield ("secret", pkgdir, f"{group}/{name}")
            elif os.path.exists(os.path.join(pkgdir, ".system")):
                yield ("system", pkgdir, f"{group}/{name}")
            else:
                yield ("link", pkgdir, f"{group}/{name}")
