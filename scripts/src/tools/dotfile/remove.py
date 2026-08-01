import os
import shutil
import subprocess

from tools.dotfile.link import unfold
from tools.dotfile.packages import (
    load_package_metadata,
    package_groups,
    sync_packages,
    validate_package_names,
)
from tools.dotfile.state import (
    canon,
    die,
    log,
    owned_by_repo,
    resolve_link,
    shorten,
    sorted_entries,
    trim,
)
from tools.dotfile.targets import has_target_under, load_targets, map_dst, never_fold


def locate_remove_source(ctx, source_input):
    if source_input == ctx.root:
        die("path must include a package")
    if source_input.startswith(ctx.root + "/"):
        rel = source_input[len(ctx.root) + 1 :]
    elif source_input.startswith("/"):
        rel = source_input[1:]
    elif source_input.startswith("./"):
        rel = source_input[2:]
    else:
        rel = source_input
    rel = canon("/" + rel)[1:]
    if not rel:
        die("path must include a package")

    best = ""
    for group in package_groups(ctx):
        if rel.startswith(group + "/") and len(group) > len(best):
            best = group
    if not best:
        die(f"not a package path: {source_input}")

    rest = rel[len(best) + 1 :]
    pkg = rest.split("/", 1)[0]
    if not pkg:
        die("path must include a package")
    if pkg == "overrides":
        die("override paths cannot be removed as packages")

    source = os.path.join(ctx.root, rel)
    if not os.path.exists(source) and not os.path.islink(source):
        die(f"not found in dotfiles: {rel}")
    package_root = os.path.join(ctx.root, best, pkg)
    if not os.path.isdir(package_root):
        die(f"not a package path: {rel}")

    return best, pkg, package_root, rel, source


def remove_destination(ctx, full, pkg, rel):
    destination = map_dst(ctx, full, pkg, rel)
    if owned_by_repo(ctx, destination):
        die(f"target for {full} points inside dotfiles")
    return destination


def existing_remove_parent(destination):
    parent = os.path.dirname(destination)
    while not os.path.exists(parent) and not os.path.islink(parent):
        if parent == "/":
            break
        parent = os.path.dirname(parent)
    return parent


def validate_remove_node(ctx, source, full, pkg, rel):
    remove_destination(ctx, full, pkg, rel)
    if not os.path.isdir(source) or os.path.islink(source):
        return
    for name in sorted_entries(source):
        entry_rel = f"{rel}/{name}" if rel else name
        validate_remove_node(ctx, os.path.join(source, name), f"{full}/{name}", pkg, entry_rel)


def discard_remove_source(ctx, source, destination):
    if os.path.isdir(source) and not os.path.islink(source):
        shutil.rmtree(source)
    else:
        os.remove(source)
    log(f"kept   existing {shorten(ctx, destination)}")


def unfold_remove_ancestors(ctx, source, destination):
    parent = os.path.dirname(destination)
    current = "/"
    for segment in parent.strip("/").split("/"):
        if not segment:
            continue
        current = current.rstrip("/") + "/" + segment
        if not os.path.islink(current):
            continue
        resolved = resolve_link(current)
        if source.startswith(resolved + "/"):
            if not os.path.isdir(resolved):
                die(f"managed parent is not a directory: {shorten(ctx, current)}")
            unfold(ctx, current, resolved)


def materialize_remove_node(ctx, source, full, pkg, rel):
    destination = remove_destination(ctx, full, pkg, rel)
    unfold_remove_ancestors(ctx, source, destination)

    if os.path.isdir(source) and not os.path.islink(source):
        if os.path.islink(destination):
            current = resolve_link(destination)
            if current != source:
                discard_remove_source(ctx, source, destination)
                return
            if has_target_under(ctx, full) or never_fold(ctx, destination):
                unfold(ctx, destination, source)
            else:
                os.remove(destination)
                shutil.move(source, destination)
                log(f"kept   {shorten(ctx, destination)}")
                return
        elif os.path.exists(destination) and not os.path.isdir(destination):
            discard_remove_source(ctx, source, destination)
            return
        if not os.path.exists(destination):
            parent = existing_remove_parent(destination)
            if not os.path.isdir(parent):
                discard_remove_source(ctx, source, parent)
                return
            if not has_target_under(ctx, full) and not never_fold(ctx, destination):
                os.makedirs(os.path.dirname(destination), exist_ok=True)
                shutil.move(source, destination)
                log(f"kept   {shorten(ctx, destination)}")
                return
            os.makedirs(destination, exist_ok=True)
        for name in sorted_entries(source):
            entry_rel = f"{rel}/{name}" if rel else name
            materialize_remove_node(
                ctx, os.path.join(source, name), f"{full}/{name}", pkg, entry_rel
            )
        os.rmdir(source)
        return

    if os.path.islink(destination):
        current = resolve_link(destination)
        if current == source:
            os.remove(destination)
        else:
            discard_remove_source(ctx, source, destination)
            return
    elif os.path.exists(destination):
        discard_remove_source(ctx, source, destination)
        return
    else:
        parent = existing_remove_parent(destination)
        if not os.path.isdir(parent):
            discard_remove_source(ctx, source, parent)
            return
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    shutil.move(source, destination)
    log(f"kept   {shorten(ctx, destination)}")


def remove_target_entries(ctx, prefix):
    if not os.path.isfile(ctx.targets_file):
        return
    with open(ctx.targets_file, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    kept = []
    changed = False
    for raw in lines:
        if "=" in raw:
            key = trim(raw.split("=", 1)[0])
            if key == prefix or key.startswith(prefix + "/"):
                changed = True
                log(f"unmapped {key}")
                continue
        kept.append(raw)
    if changed:
        with open(ctx.targets_file, "w", encoding="utf-8") as handle:
            handle.write("".join(line + "\n" for line in kept))


def prune_empty_package_dirs(ctx, directory, package_root):
    while directory == package_root or directory.startswith(package_root + "/"):
        if os.path.isdir(directory):
            try:
                os.rmdir(directory)
            except OSError:
                return
        if directory == package_root:
            return
        directory = os.path.dirname(directory)


def cmd_remove(ctx, path):
    _group, pkg, package_root, rel, source = locate_remove_source(ctx, path)
    load_targets(ctx)
    load_package_metadata(ctx)
    validate_package_names(ctx)

    if source == package_root:
        node_rel = ""
    else:
        node_rel = source[len(package_root) + 1 :]

    validate_remove_node(ctx, source, rel, pkg, node_rel)
    source_parent = os.path.dirname(source)
    materialize_remove_node(ctx, source, rel, pkg, node_rel)
    prune_empty_package_dirs(ctx, source_parent, package_root)
    remove_target_entries(ctx, rel)

    sync_packages(ctx)
    subprocess.run(
        [
            "git",
            "-C",
            ctx.root,
            "add",
            "-A",
            "--",
            rel,
            ctx.targets_file,
            ctx.packages_config,
            ctx.packages_doc,
        ],
        stderr=subprocess.DEVNULL,
        check=False,
    )
    log(f"removed {rel} from dotfiles")
