"""Overlay merges: a platform package's settings.macos.json deep-merges into the
same-named earlier package's settings.json, and the merged result is materialised
at the base file's destination instead of a symlink.

The destination is a real file that its application also writes, so the merge is a
three-way one: the repo's document, the live file, and the last document we
materialised there (the baseline). Comparison is on parsed documents, never on
bytes, so an editor reformatting the file it owns is not a change. Drift means the
file has changes nobody has decided yet; everything else applies quietly.
"""

import json
import os

from tools.dotfile import baseline, jsonc, mergeconf
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
FORMATTING = "formatting"
PENDING = "pending"
DRIFTED = "drifted"
CONFLICT = "conflict"
MISSING = "missing"

BLOCKED = (DRIFTED, CONFLICT)

ADD = "add"
MODIFY = "modify"
DELETE = "delete"

SKIP = "skip"
REPO = "repo"
LIVE = "live"

MARKERS = (".nolink", ".secret", ".system")

DETAIL_KEYS = 5

UNSET = object()  # a key (or a whole document) that is not there at all


class Entry:
    def __init__(self, base, operations, dst, ignores=(), base_dir=""):
        self.base = base
        self.operations = operations
        self.dst = dst
        self.ignores = list(ignores)
        self.base_dir = base_dir  # the package directory holding the base file

    def document(self, ctx):
        document = None
        for kind, path in self.operations:
            parsed = read_json(ctx, path)
            if kind == "plain":
                document = parsed
            else:
                document = deep_merge(document, parsed)
        return document

    def content(self, ctx):
        return render(self.document(ctx))

    def paths(self):
        return [path for _kind, path in self.operations]


class Change:
    """One key the repo and the live file disagree about.

    `ours` is the repo's value, `theirs` the live one, `base` the last materialised
    one; any of them may be UNSET when that side has no such key.
    """

    def __init__(self, kind, path, ours, theirs, base):
        self.kind = kind
        self.path = path
        self.ours = ours
        self.theirs = theirs
        self.base = base

    def key(self):
        return "/".join(self.path)


class Review:
    """What sits at a destination now, and what belongs there."""

    def __init__(self, state, detail, document, changes, live, raw):
        self.state = state
        self.detail = detail
        self.document = document
        self.changes = changes
        self.live = live
        self.raw = raw


def render(document):
    return json.dumps(document, indent=4, ensure_ascii=False) + "\n"


def read_json(ctx, path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    try:
        return jsonc.loads(text)
    except ValueError as error:
        die(f"merge: {shorten(ctx, path)} is not valid JSONC: {error}")


def read_live(ctx, entry):
    """(raw bytes, document) at the destination; the document is UNSET if it will not parse."""
    with open(entry.dst, "rb") as handle:
        raw = handle.read()
    try:
        return raw, jsonc.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        return raw, UNSET


def deep_merge(base, overlay):
    """Objects merge recursively (the overlay wins scalars); anything else replaces."""
    if isinstance(base, dict) and isinstance(overlay, dict):
        merged = dict(base)
        for key, value in overlay.items():
            merged[key] = deep_merge(base[key], value) if key in base else value
        return merged
    return overlay


def child(value, key):
    return value[key] if isinstance(value, dict) and key in value else UNSET


def branching(ours, theirs):
    """Both sides are objects (or absent), so the merge can descend key by key."""
    if isinstance(ours, dict):
        return isinstance(theirs, dict) or theirs is UNSET
    return ours is UNSET and isinstance(theirs, dict)


def union_keys(ours, theirs):
    """Repo order first, then whatever this machine added on the end."""
    keys = list(ours) if isinstance(ours, dict) else []
    seen = set(keys)
    if isinstance(theirs, dict):
        keys.extend(key for key in theirs if key not in seen)
    return keys


def classify(ours, theirs, base, tracked):
    """(kind, value) for one key; the kind is None when the repo's value applies quietly.

    Without a baseline (`tracked` false) nothing can be attributed to either side, so
    every difference is a change. That happens once, on the run that records the first
    baseline.
    """
    if ours == theirs:
        return None, ours
    if not tracked:
        return (ADD if ours is UNSET or theirs is UNSET else MODIFY), theirs
    if theirs is UNSET:
        return (None, ours) if base is UNSET else (DELETE, theirs)
    if ours is UNSET:
        return (ADD, theirs) if base is UNSET else (None, UNSET)
    if theirs == base:
        return None, ours
    if ours == base:
        return MODIFY, theirs
    return CONFLICT, theirs


def walk(path, ours, theirs, base, tracked, ignores, decisions, changes):
    if path and mergeconf.matches(path, ignores):
        return theirs
    if branching(ours, theirs):
        merged = {}
        for key in union_keys(ours, theirs):
            value = walk(
                path + (key,),
                child(ours, key),
                child(theirs, key),
                child(base, key),
                tracked,
                ignores,
                decisions,
                changes,
            )
            if value is not UNSET:
                merged[key] = value
        if merged or ours == {} or theirs == {}:
            return merged
        return UNSET
    kind, value = classify(ours, theirs, base, tracked)
    if kind is None:
        return value
    changes.append(Change(kind, path, ours, theirs, base))
    return ours if decisions.get(path) == REPO else theirs


def resolve(ours, theirs, base, ignores, decisions=None):
    """Three-way merge the repo document against the live one: (document, changes).

    `base` is None when no baseline was recorded, which falls back to a two-way
    compare. Keys nobody has decided are held at their live value, so materialising
    the document never clobbers a pending decision. Ignored keys are passed through
    from the live file verbatim and are never changes.
    """
    changes = []
    document = walk(
        (),
        ours,
        theirs,
        UNSET if base is None else base,
        base is not None,
        ignores,
        decisions or {},
        changes,
    )
    if document is UNSET:  # every key resolved away
        document = {}
    return document, changes


def state_of(document, live, raw, changes, decisions):
    undecided = [change for change in changes if change.path not in decisions]
    if any(change.kind == CONFLICT for change in undecided):
        return CONFLICT
    if undecided:
        return DRIFTED
    if document != live:
        return PENDING
    return CURRENT if raw == render(document).encode("utf-8") else FORMATTING


def detail_of(state, changes):
    if state == FORMATTING:
        return "same content, reformatted in place"
    if not changes:
        return ""
    named = [change.key() or "(document)" for change in changes[:DETAIL_KEYS]]
    rest = len(changes) - len(named)
    return ", ".join(named) + (f" (+{rest} more)" if rest else "")


def review(ctx, entry, decisions=None):
    """Classify the destination and work out the document that belongs there."""
    ours = entry.document(ctx)
    if os.path.islink(entry.dst):
        if owned_by_repo(ctx, resolve_link(entry.dst)):
            return Review(PENDING, "symlink where a merged file belongs", ours, [], UNSET, b"")
        return Review(DRIFTED, "foreign symlink where a merged file belongs", ours, [], UNSET, b"")
    if not os.path.exists(entry.dst):
        return Review(MISSING, "", ours, [], UNSET, b"")
    if os.path.isdir(entry.dst):
        return Review(DRIFTED, "directory where a merged file belongs", ours, [], UNSET, b"")
    raw, live = read_live(ctx, entry)
    if live is UNSET:
        return Review(DRIFTED, "not valid JSON", ours, [], UNSET, raw)
    base = baseline.load(ctx, entry.dst)
    document, changes = resolve(ours, live, base, entry.ignores, decisions)
    state = state_of(document, live, raw, changes, decisions or {})
    return Review(state, detail_of(state, changes), document, changes, live, raw)


def write(ctx, entry, content):
    os.makedirs(os.path.dirname(entry.dst), exist_ok=True)
    if os.path.islink(entry.dst):
        os.remove(entry.dst)  # never write through a link into the repo
    with open(entry.dst, "w", encoding="utf-8") as handle:
        handle.write(content)


def materialise(ctx, entry, document, live, raw, dry):
    """Write the document unless the file already carries it, then record the baseline."""
    if document == live:
        state = CURRENT if raw == render(document).encode("utf-8") else FORMATTING
    else:
        state = WROTE
        if not dry:
            write(ctx, entry, render(document))
    if not dry:
        baseline.save(ctx, entry.dst, document)
    return state


def decide(ctx, entry, changes, resolution, resolver):
    """Which side wins each changed key: {path: REPO|LIVE}. An absent path stays undecided."""
    if resolver is not None:
        return resolver(ctx, entry, changes) or {}
    if resolution == SKIP:
        return {}
    return {change.path: resolution for change in changes}


def remember_adopted(ctx, entry, changes, decisions):
    """Keys the live file won: the repo copy still has to be taught about them."""
    taken = [change for change in changes if decisions.get(change.path) == LIVE]
    if taken:
        ctx.merge_adopted.append((entry, taken))


def settle(ctx, entry, dry, force=False, resolution=SKIP, resolver=None):
    """Materialise one entry: (state, the changes still waiting on a decision).

    `resolver` is called as resolver(ctx, entry, changes) and returns the decisions to
    apply; without one the resolution mode decides the lot.
    """
    if force:
        resolution = REPO
    found = review(ctx, entry)
    if found.state in BLOCKED and not found.changes:
        # a foreign symlink or unparseable file: nothing to merge with, only the repo can
        # win. A directory is never cleared away; that is for a person to sort out.
        if resolution != REPO or os.path.isdir(entry.dst):
            return found.state, []
        return materialise(ctx, entry, found.document, UNSET, b"", dry), []
    decisions = decide(ctx, entry, found.changes, resolution, resolver) if found.changes else {}
    if decisions:
        found = review(ctx, entry, decisions)
    if found.state in BLOCKED:
        return found.state, [change for change in found.changes if change.path not in decisions]
    remember_adopted(ctx, entry, found.changes, decisions)
    return materialise(ctx, entry, found.document, found.live, found.raw, dry), []


def inspect(ctx, entry):
    found = review(ctx, entry)
    return found.state, found.detail


def key_changes(ctx, entry):
    """The undecided changes at a destination, for callers that render key-level detail."""
    return review(ctx, entry).changes


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
            if name in MARKERS or name == mergeconf.NAME:
                continue
            src = os.path.join(parent, name)
            if vault_owned(src):
                continue
            files[src[len(pkgdir) + 1 :]] = src
    return files


def entry_pkgdirs(ops):
    found = []
    for op in ops:
        if op[4] not in found:
            found.append(op[4])
    return found


def load(ctx):
    """Find overlay merges among the active packages; store entries and skip paths."""
    ctx.merge_entries = []
    ctx.merge_paths = set()
    ctx.merge_adopted = []
    ctx.merge_choice = {}
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
                records.setdefault((pkg, rel), []).append((index, "plain", src, name, pkgdir))
                continue
            if base_rel in files:
                die(f"merge: {name} carries both {base_rel} and overlay {rel}")
            records.setdefault((pkg, base_rel), []).append((index, "overlay", src, name, pkgdir))
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
        entry = Entry(
            base,
            [(kind, src) for _index, kind, src, _group, _dir in ops],
            map_dst(ctx, base, pkg, base_rel),
            mergeconf.load_ignores(entry_pkgdirs(ops)),
            providers[0][4],
        )
        ctx.merge_entries.append(entry)
        ctx.merge_paths.update(entry.paths())
    ctx.merge_entries.sort(key=lambda entry: entry.dst)


def report_blocked(changes):
    for change in changes[:DETAIL_KEYS]:
        log(f"    {change.kind}: {change.key() or '(document)'}")
    rest = len(changes) - DETAIL_KEYS
    if rest > 0:
        log(f"    ... and {rest} more")


def apply_entries(ctx, dry, force=False, resolution=SKIP, resolver=None):
    if not ctx.merge_entries:
        return False
    blocked = False
    verb = "would merge" if dry else "merge"
    for entry in ctx.merge_entries:
        state, changes = settle(ctx, entry, dry, force, resolution, resolver)
        log(f"  {verb} {shorten(ctx, entry.dst)} ({state})")
        if state in BLOCKED:
            blocked = True
            report_blocked(changes)
    if blocked:
        log("  drifted: run dotfile sync in a terminal to settle these key by key,")
        log("  or --resolve repo to discard them / --resolve live to adopt them")
    return blocked


def cmd_merge(ctx, profile, dry, force, resolution=SKIP):
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
    if apply_entries(ctx, dry, force, resolution):
        raise SystemExit(1)
