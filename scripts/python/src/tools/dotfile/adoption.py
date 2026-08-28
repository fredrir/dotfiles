"""Routing a locally-made change back into the repository.

`merge` decides which side of a key wins; it does not know where a key the live
file won belongs in the repo. This module answers that: on a terminal by asking,
and otherwise by following the layer that already defines the key.
"""

import os
import sys

from tools.dotfile import adopt, layers, mergeconf, render, select
from tools.dotfile import merge as merge_state
from tools.dotfile.state import die, log, shorten

IGNORE = "ignore"
DISCARD = "discard"

JSON = ".json"


def interactive():
    return sys.stdin.isatty() and sys.stdout.isatty()


def overlay_file(ctx, entry, group, tag):
    """Where an overlay for `tag` belongs: <group>/<package>/<stem>.<tag>.json.

    merge reads the tag off the group directory as well as the filename, so an
    overlay only counts when it sits in the group its own name points at.
    """
    base = entry.paths()[0]
    rel = os.path.relpath(base, entry.base_dir)
    stem = rel.removesuffix(JSON)
    pkg = os.path.basename(entry.base_dir)
    return os.path.join(ctx.root, group, pkg, f"{stem}.{tag}{JSON}")


def overlay_slots(ctx, entry):
    """Every platform layer this key could go to, as tag -> repo file.

    The offer is drawn from the groups this profile actually links, because the
    tag is a group directory's basename: linux/arch is reachable as 'arch', and
    'linux' is not a tag at all.
    """
    slots = {}
    for group in ctx.link_groups:
        if "/overrides/" in group:
            continue
        tag = group.rsplit("/", 1)[-1]
        if tag != layers.SHARED and tag not in slots:
            slots[tag] = overlay_file(ctx, entry, group, tag)
    for path_to_file in entry.paths():  # one that already exists beats a derived path
        name = layers.layer_name(path_to_file, ctx.root)
        if name != layers.SHARED:
            slots[name] = layers.resolve(path_to_file, ctx.root)
    return slots


def shown(value):
    return None if value is merge_state.UNSET else value


def render_change(change, width=None):
    """The value side of one change, written out for a person to read."""
    return render.change(change.kind, shown(change.ours), shown(change.theirs), width)


def described(change, targets):
    return select.Change(
        change.kind, change.path, render.key(change.path), render_change(change), targets
    )


def resolver(ctx, entry, changes):
    """Ask where each changed key belongs; None from the selector means nothing decided."""
    targets = sorted(overlay_slots(ctx, entry))
    picked = select.resolve(
        shorten(ctx, entry.dst), [described(change, targets) for change in changes]
    )
    if picked is None:
        return None
    decisions = {}
    for index, decision in picked.items():
        change = changes[index]
        decisions[change.path] = merge_state.REPO if decision == DISCARD else merge_state.LIVE
        ctx.merge_choice[(entry.dst, change.path)] = decision
    return decisions


def layer_for(ctx, entry, path, decision):
    """The repo file that should carry this key. No decision means the one that owns it."""
    files = entry.paths()
    if decision is None or decision == layers.SHARED:
        chosen = None if decision else layers.owning_layer(files, path, ctx.root)
        return chosen or layers.target_layer(files, layers.SHARED, ctx.root)
    name = layers.decision_name(decision)
    slot = overlay_slots(ctx, entry).get(name)
    if slot is None:
        die(f"no layer named '{name}' for {shorten(ctx, entry.dst)}")
    return slot


def write_back(ctx):
    """Teach the repo about every key the live file won."""
    for entry, taken in getattr(ctx, "merge_adopted", []):
        for change in taken:
            decision = ctx.merge_choice.get((entry.dst, change.path))
            if decision == IGNORE:
                target = os.path.join(entry.base_dir, mergeconf.NAME)
                verb = "would ignore" if ctx.dry else "ignore"
                if not ctx.dry:
                    layers.add_ignore(target, change.path)
            else:
                target = layer_for(ctx, entry, change.path, decision)
                verb = "would adopt" if ctx.dry else "adopt"
                if not ctx.dry:
                    if change.theirs is merge_state.UNSET:
                        adopt.remove_key(target, change.path)
                    else:
                        adopt.set_key(target, change.path, change.theirs)
            log(f"  {verb} {change.key() or '(document)'} → {shorten(ctx, target)}")
