import glob
import os
import shutil
import subprocess

from tools.dotfile.packages import load_package_metadata, sync_packages
from tools.dotfile.state import (
    canon,
    die,
    log,
    manifest_groups,
    owned_by_repo,
    resolve_link,
    resolve_profile,
    shorten,
)
from tools.dotfile.targets import load_targets


def locate_source(ctx, path):
    if os.path.exists(path) or os.path.islink(path):
        if path.startswith("/"):
            return path
        return os.path.join(os.getcwd(), path)
    config_path = os.path.join(ctx.home, ".config", path)
    if os.path.exists(config_path):
        return config_path
    prefix = os.path.join(ctx.home, ".config") + "/"
    matches = sorted(glob.glob(glob.escape(config_path) + "*"))
    if len(matches) == 1:
        return matches[0]
    if len(matches) > 1:
        names = " ".join(match[len(prefix) :] for match in matches)
        die(f"ambiguous, matches: {names}")
    die(f"not found: {path} (looked in ~/.config)")


def repo_links_under(ctx, directory):
    found = []
    for parent, dirnames, filenames in os.walk(directory):
        for name in list(dirnames) + list(filenames):
            full = os.path.join(parent, name)
            if os.path.islink(full):
                found.append(full)
    return sorted(found)


def refuse_managed_source(ctx, src):
    if os.path.islink(src):
        if owned_by_repo(ctx, resolve_link(src)):
            die(f"already managed: {shorten(ctx, src)}")
        die(f"refusing to adopt a foreign symlink: {shorten(ctx, src)}")
    if os.path.isdir(src):
        for link in repo_links_under(ctx, src):
            if owned_by_repo(ctx, resolve_link(link)):
                die(
                    f"already partially managed ({shorten(ctx, link)}), add individual files instead"
                )


def plan_destination(ctx, src, group, pkgflag):
    config_prefix = os.path.join(ctx.home, ".config") + "/"
    mapline = ""
    if src.startswith(config_prefix):
        rel = src[len(config_prefix) :]
        if "/" in rel:
            pkg = pkgflag or rel.split("/", 1)[0]
            destrel = f"{group}/{pkg}/{rel.split('/', 1)[1]}"
        elif os.path.isdir(src):
            pkg = pkgflag or rel
            destrel = f"{group}/{pkg}"
            if os.path.exists(os.path.join(ctx.root, destrel)):
                die(f"package exists: {destrel} (add files inside it individually)")
        else:
            pkg = pkgflag or rel.split(".", 1)[0]
            destrel = f"{group}/{pkg}/{rel}"
            mapline = f"{group}/{pkg} = ~/.config"
    elif src.startswith(ctx.home + "/"):
        if not pkgflag:
            die("files outside ~/.config need --pkg <name>")
        pkg = pkgflag
        destrel = f"{group}/{pkg}/{os.path.basename(src)}"
        mapline = f"{destrel} = {shorten(ctx, src)}"
    else:
        die("source must live under $HOME")
    return pkg, destrel, mapline


def warn_if_group_unlinked(ctx, group):
    profile = resolve_profile(ctx, "")
    manifest = os.path.join(ctx.environment_dir, profile, "manifest")
    if not os.path.isfile(manifest):
        return
    if group not in manifest_groups(manifest):
        log(
            f"note: group '{group}' is not in environment/{profile}/manifest, "
            "it will not be linked by 'dotfile link' on this machine"
        )


def git_add(ctx, *paths):
    subprocess.run(
        ["git", "-C", ctx.root, "add", *paths],
        stderr=subprocess.DEVNULL,
        check=False,
    )


def cmd_add(ctx, path, group, pkgflag, description):
    if "\n" in description or "\r" in description:
        die("description must be a single line")

    expanded = ctx.home + path[1:] if path.startswith("~") else path
    src = canon(locate_source(ctx, expanded))
    refuse_managed_source(ctx, src)

    load_targets(ctx)
    pkg, destrel, mapline = plan_destination(ctx, src, group, pkgflag)

    dest = os.path.join(ctx.root, destrel)
    if os.path.exists(dest):
        die(f"destination exists: {destrel}")

    load_package_metadata(ctx)
    if description:
        ctx.package_descriptions[f"{group}/{pkg}"] = description

    os.makedirs(os.path.dirname(dest), exist_ok=True)
    shutil.move(src, dest)
    os.symlink(dest, src)
    log(f"moved  {shorten(ctx, src)} -> {destrel}")
    log(f"linked {shorten(ctx, src)}")

    if mapline and not targets_has_line(ctx, mapline):
        with open(ctx.targets_file, "a", encoding="utf-8") as handle:
            handle.write(mapline + "\n")
        log(f"mapped {mapline}")

    git_add(ctx, dest, ctx.targets_file)
    sync_packages(ctx)
    git_add(ctx, ctx.packages_config, ctx.packages_doc)

    warn_if_group_unlinked(ctx, group)


def targets_has_line(ctx, line):
    try:
        with open(ctx.targets_file, encoding="utf-8") as handle:
            return line in handle.read().splitlines()
    except OSError:
        return False
