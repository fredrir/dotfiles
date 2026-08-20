"""Overlay merges: a platform package's settings.macos.json deep-merges into the
same-named earlier package's settings.json, and the merged result is materialised
at the base file's destination instead of a symlink."""

import json
import os

from tools.dotfile import jsonc
from tools.dotfile.secret.vault import vault_owned
from tools.dotfile.state import (
    collect_groups,
    die,
    each_package,
    load_overrides,
    log,
    owned_by_repo,
    require_manifest,
    resolve_link,
    resolve_profile,
    shorten,
)
from tools.dotfile.targets import load_targets, map_dst

WROTE = "wrote"
CURRENT = "current"
DRIFTED = "drifted"

MARKERS = (".nolink", ".secret", ".system")


class Entry:
    def __init__(self, base, operations, dst):
        self.base = base
        self.operations = operations
        self.dst = dst

    def content(self, ctx):
        document = None
        for kind, path in self.operations:
            parsed = read_json(ctx, path)
            if kind == "plain":
                document = parsed
            else:
                document = deep_merge(document, parsed)
        return json.dumps(document, indent=4, ensure_ascii=False) + "\n"

    def paths(self):
        return [path for _kind, path in self.operations]


def read_json(ctx, path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    try:
        return jsonc.loads(text)
    except ValueError as error:
        die(f"merge: {shorten(ctx, path)} is not valid JSONC: {error}")


def deep_merge(base, overlay):
    """Objects merge recursively (the overlay wins scalars); anything else replaces."""
    if isinstance(base, dict) and isinstance(overlay, dict):
        merged = dict(base)
        for key, value in overlay.items():
            merged[key] = deep_merge(base[key], value) if key in base else value
        return merged
    return overlay


def group_tag(name):
    """The overlay tag of a package path: its group directory's basename."""
    group = name.rsplit("/", 1)[0]
    return group.rsplit("/", 1)[-1]


def overlay_target(rel, tag):
    """settings.macos.json with the macos group tag resolves to settings.json."""
    suffix = f".{tag}.json"
    if not rel.endswith(suffix):
        return None
    stem = rel[: -len(suffix)]
    if not stem or stem.endswith("/"):
        return None
    return stem + ".json"


def package_files(pkgdir):
    files = {}
    for parent, _dirnames, names in os.walk(pkgdir):
        for name in sorted(names):
            if name in MARKERS:
                continue
            src = os.path.join(parent, name)
            if vault_owned(src):
                continue
            files[src[len(pkgdir) + 1 :]] = src
    return files


def load(ctx):
    """Find overlay merges among the active packages; store entries and skip paths."""
    ctx.merge_entries = []
    ctx.merge_paths = set()
    records = {}
    for index, (state, pkgdir, name) in enumerate(each_package(ctx)):
        if state != "link":
            continue
        pkg = os.path.basename(pkgdir)
        tag = group_tag(name)
        files = package_files(pkgdir)
        for rel, src in sorted(files.items()):
            base_rel = overlay_target(rel, tag)
            if base_rel is None:
                records.setdefault((pkg, rel), []).append((index, "plain", src, name))
                continue
            if base_rel in files:
                die(f"merge: {name} carries both {base_rel} and overlay {rel}")
            records.setdefault((pkg, base_rel), []).append((index, "overlay", src, name))
    for (pkg, base_rel), ops in sorted(records.items()):
        overlays = [op for op in ops if op[1] == "overlay"]
        if not overlays:
            continue
        providers = [op for op in ops if op[1] == "plain"]
        if not providers or providers[0][0] > overlays[0][0]:
            die(
                f"merge: overlay '{pkg}/{base_rel}' has no {base_rel} "
                "in an earlier package of the same name"
            )
        base_group = providers[0][3].rsplit("/", 1)[0]
        base = f"{base_group}/{pkg}/{base_rel}"
        entry = Entry(base, [(kind, src) for _index, kind, src, _group in ops], map_dst(ctx, base, pkg, base_rel))
        ctx.merge_entries.append(entry)
        ctx.merge_paths.update(entry.paths())
    ctx.merge_entries.sort(key=lambda entry: entry.dst)


def write(ctx, entry, content):
    os.makedirs(os.path.dirname(entry.dst), exist_ok=True)
    with open(entry.dst, "w", encoding="utf-8") as handle:
        handle.write(content)


def settle(ctx, entry, dry, force):
    content = entry.content(ctx)
    if os.path.islink(entry.dst):
        current = resolve_link(entry.dst)
        if not owned_by_repo(ctx, current):
            return DRIFTED
        if not dry:
            os.remove(entry.dst)
            write(ctx, entry, content)
        return WROTE
    if os.path.exists(entry.dst):
        with open(entry.dst, "rb") as handle:
            if handle.read() != content.encode("utf-8"):
                if not force:
                    return DRIFTED
                if not dry:
                    write(ctx, entry, content)
                return WROTE
        return CURRENT
    if not dry:
        write(ctx, entry, content)
    return WROTE


def apply_entries(ctx, dry, force):
    if not ctx.merge_entries:
        return False
    blocked = False
    verb = "would merge" if dry else "merge"
    for entry in ctx.merge_entries:
        state = settle(ctx, entry, dry, force)
        log(f"  {verb} {shorten(ctx, entry.dst)} ({state})")
        if state == DRIFTED:
            blocked = True
    if blocked:
        log("  drifted: edit the repo copy (or an overlay) to adopt local changes,")
        log("  or re-run with --force to discard them")
    return blocked


def inspect(ctx, entry):
    content = entry.content(ctx)
    if not os.path.exists(entry.dst) and not os.path.islink(entry.dst):
        return "missing", ""
    if os.path.islink(entry.dst):
        return "differs", "symlink where a merged file belongs"
    with open(entry.dst, "rb") as handle:
        if handle.read() != content.encode("utf-8"):
            return "differs", "edited locally"
    return "ok", ""


def cmd_merge(ctx, profile, dry, force):
    ctx.dry = dry
    profile = resolve_profile(ctx, profile or "")
    manifest = require_manifest(ctx, profile)
    load_targets(ctx)
    load_overrides(ctx)
    collect_groups(ctx, manifest)
    load(ctx)
    if not ctx.merge_entries:
        log("no overlays to merge")
        return
    if apply_entries(ctx, dry, force):
        raise SystemExit(1)
