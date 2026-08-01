import os

from tools.dotfile.state import (
    collect_groups,
    die,
    each_package,
    load_overrides,
    log,
    owned_by_repo,
    require_manifest,
    resolve_link,
    resolve_path,
    resolve_profile,
    run_op,
    save_overrides,
    save_profile,
    select_override,
    shorten,
    sorted_entries,
)
from tools.dotfile.targets import has_target_under, load_targets, map_dst, never_fold

PRUNE_DEPTH = 6


def conflict(ctx, path):
    ctx.conflicts.append(path)


def remove_path(ctx, target):
    run_op(ctx, ["rm", target], lambda: os.remove(target))


def make_dirs(ctx, target):
    run_op(ctx, ["mkdir", "-p", target], lambda: os.makedirs(target, exist_ok=True))


def make_link(ctx, source, target):
    run_op(ctx, ["ln", "-s", source, target], lambda: os.symlink(source, target))


def link_file(ctx, src, dst):
    if os.path.islink(dst):
        current = resolve_link(dst)
        if current == src:
            return
        if owned_by_repo(ctx, current):
            remove_path(ctx, dst)
        else:
            conflict(ctx, dst)
            return
    elif os.path.exists(dst):
        conflict(ctx, dst)
        return
    make_dirs(ctx, os.path.dirname(dst))
    make_link(ctx, src, dst)
    log(f"  link {shorten(ctx, dst)}")


def unfold(ctx, dst, current):
    remove_path(ctx, dst)
    make_dirs(ctx, dst)
    for name in sorted_entries(current):
        make_link(ctx, os.path.join(current, name), os.path.join(dst, name))
    log(f"  split {shorten(ctx, dst)}")


def link_dir(ctx, src, dst, full, pkg, rel):
    if os.path.islink(dst):
        current = resolve_link(dst)
        if current == src:
            if has_target_under(ctx, full):
                unfold(ctx, dst, current)
            else:
                return
        elif owned_by_repo(ctx, current):
            if os.path.isdir(current):
                unfold(ctx, dst, current)
            else:
                remove_path(ctx, dst)
        else:
            conflict(ctx, dst)
            return
    if not os.path.exists(dst):
        if has_target_under(ctx, full) or never_fold(ctx, dst):
            make_dirs(ctx, dst)
        else:
            make_dirs(ctx, os.path.dirname(dst))
            make_link(ctx, src, dst)
            log(f"  link {shorten(ctx, dst)}")
            return
    elif not os.path.isdir(dst):
        conflict(ctx, dst)
        return
    for name in sorted_entries(src):
        entry_rel = f"{rel}/{name}" if rel else name
        walk_node(ctx, pkg, entry_rel, os.path.join(src, name), f"{full}/{name}")


def walk_node(ctx, pkg, rel, src, full):
    if os.path.basename(src) == ".nolink":
        return
    dst = map_dst(ctx, full, pkg, rel)
    if os.path.isdir(src) and not os.path.islink(src):
        link_dir(ctx, src, dst, full, pkg, rel)
    else:
        link_file(ctx, src, dst)


def link_package(ctx, pkgdir, name):
    walk_node(ctx, os.path.basename(pkgdir), "", pkgdir, name)


def stale_override_link(ctx, current):
    if "/overrides/" not in current:
        return False
    base = current.split("/overrides/", 1)[0]
    if not os.path.isdir(base + "/overrides"):
        return False
    for active in ctx.active_override_dirs:
        if current.startswith(active + "/"):
            return False
    return True


def collect_repo_links(path, depth, max_depth, prefix, found):
    if os.path.islink(path):
        try:
            raw = os.readlink(path)
        except OSError:
            return
        if raw.startswith(prefix):
            found.append(path)
        return
    if depth >= max_depth or not os.path.isdir(path):
        return
    for name in sorted_entries(path):
        collect_repo_links(os.path.join(path, name), depth + 1, max_depth, prefix, found)


def prune_candidates(ctx):
    prefix = ctx.root + "/"
    found = []
    for name in sorted_entries(ctx.home):
        full = os.path.join(ctx.home, name)
        if os.path.islink(full):
            try:
                raw = os.readlink(full)
            except OSError:
                continue
            if raw.startswith(prefix):
                found.append(full)
    for start in (os.path.join(ctx.home, ".config"), os.path.join(ctx.home, ".local")):
        collect_repo_links(start, 0, PRUNE_DEPTH, prefix, found)
    return found


def prune(ctx):
    for link in prune_candidates(ctx):
        current = resolve_link(link)
        if not owned_by_repo(ctx, current):
            continue
        if not stale_override_link(ctx, current) and os.path.exists(link):
            continue
        remove_path(ctx, link)
        log(f"  prune {shorten(ctx, link)}")


def report_conflicts(ctx, profile):
    if not ctx.conflicts:
        return
    log("")
    log("conflicts (existing files not owned by dotfiles):")
    for path in ctx.conflicts:
        log(f"  {path}")
    log(f"move each aside and re-run: mv <file> <file>.bak && dotfile link {profile}")
    raise SystemExit(1)


def cmd_link(ctx, profile, dry_run, override_specs):
    ctx.dry = dry_run
    profile = resolve_profile(ctx, profile or "")
    manifest = require_manifest(ctx, profile)
    load_targets(ctx)
    load_overrides(ctx)
    for spec in override_specs:
        if "=" not in spec:
            die("--override needs <group>=<name|none>")
        group, _, name = spec.partition("=")
        select_override(ctx, group, name)

    log(f"linking profile '{profile}'")
    collect_groups(ctx, manifest)
    prune(ctx)

    for state, pkgdir, name in each_package(ctx):
        if state == "link":
            link_package(ctx, pkgdir, name)
        elif state == "nolink":
            log(f"  skip (.nolink): {name}")
        elif state == "no-group":
            log(f"  skip missing group: {name}")

    save_profile(ctx, profile)
    save_overrides(ctx)
    report_conflicts(ctx, profile)
    log("done")


def status_files(pkgdir):
    found = []
    for parent, dirnames, filenames in os.walk(pkgdir):
        for name in dirnames:
            full = os.path.join(parent, name)
            if os.path.islink(full):
                found.append(full)
        for name in filenames:
            found.append(os.path.join(parent, name))
    return sorted(found)


def cmd_status(ctx, profile):
    profile = resolve_profile(ctx, profile or "")
    manifest = require_manifest(ctx, profile)
    load_targets(ctx)
    load_overrides(ctx)
    collect_groups(ctx, manifest)

    ok = missing = differing = 0
    for state, pkgdir, name in each_package(ctx):
        if state != "link":
            continue
        pkg = os.path.basename(pkgdir)
        for file in status_files(pkgdir):
            if os.path.basename(file) == ".nolink":
                continue
            rel = file[len(pkgdir) + 1 :]
            full = f"{name}/{rel}"
            dst = map_dst(ctx, full, pkg, rel)
            if not os.path.exists(dst) and not os.path.islink(dst):
                log(f"missing  {shorten(ctx, dst)}")
                missing += 1
            elif resolve_path(dst) == file:
                ok += 1
            else:
                log(f"differs  {shorten(ctx, dst)}")
                differing += 1

    log(f"profile '{profile}': {ok} linked, {missing} missing, {differing} differing")
    if missing + differing:
        raise SystemExit(1)
