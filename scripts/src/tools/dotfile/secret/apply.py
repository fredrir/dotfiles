from tools.core.console import colors_enabled
from tools.dotfile.report import DIM, GREEN, RED, YELLOW, paint
from tools.dotfile.secret.variables import load as load_vars
from tools.dotfile.secret.vault import (
    ABSENT,
    BLOCKING,
    CLEANED,
    CURRENT,
    DRIFTED,
    FAILED,
    PLAINTEXT,
    REMODED,
    SEALED,
    UNRESOLVED,
    WROTE,
    inspect,
    materialise,
    plan,
    secure_package_dirs,
    unmaterialise,
)
from tools.dotfile.state import (
    collect_groups,
    load_overrides,
    log,
    require_manifest,
    resolve_profile,
    shorten,
)
from tools.dotfile.targets import load_targets

MARKS = {
    WROTE: ("wrote", GREEN),
    REMODED: ("remoded", GREEN),
    CLEANED: ("removed", GREEN),
    CURRENT: ("current", DIM),
    SEALED: ("sealed", YELLOW),
    ABSENT: ("absent", YELLOW),
    DRIFTED: ("drifted", RED),
    FAILED: ("failed", RED),
    PLAINTEXT: ("plaintext", RED),
    UNRESOLVED: ("unresolved", RED),
}

ADVICE = {
    DRIFTED: "edited on this machine; dotfile secret edit, or apply --force to discard",
    FAILED: "no recipient here can decrypt it",
    PLAINTEXT: "unencrypted file inside a .secret package",
    SEALED: "no age identity on this machine",
    UNRESOLVED: "a template names a var that vars.enc.yaml does not define",
}

QUIET = (CURRENT,)


def prepare(ctx):
    profile = resolve_profile(ctx, "")
    manifest = require_manifest(ctx, profile)
    load_targets(ctx)
    load_overrides(ctx)
    collect_groups(ctx, manifest, notes=False)


def show(ctx, state, entry, color_on):
    label, color = MARKS[state]
    detail = f"  {paint(entry.detail, DIM, color_on)}" if entry.detail else ""
    log(
        f"  {paint(label, color, color_on):<{len('unresolved') + 12}}"
        + shorten(ctx, entry.dst)
        + detail
    )


def summarise(ctx, results, color_on, verb):
    counts = {}
    for state, _entry in results:
        counts[state] = counts.get(state, 0) + 1
    for state, _entry in results:
        if state not in QUIET:
            show(ctx, state, _entry, color_on)
    blocking = [state for state in counts if state in BLOCKING]
    for state in blocking:
        log("")
        log(f"  {MARKS[state][0]}: {ADVICE[state]}")
    if SEALED in counts and SEALED not in blocking:
        log("")
        log(f"  sealed: {ADVICE[SEALED]}")
    log(f"{verb}  {counted(counts)}")
    return bool(blocking)


def counted(counts):
    return ", ".join(f"{count} {MARKS[state][0]}" for state, count in sorted(counts.items()))


def run_apply(ctx, dry, force, quiet):
    entries = plan(ctx)
    if not entries:
        if not quiet:
            log("no secrets to apply")
        return False
    color_on = colors_enabled()
    declared = load_vars(ctx)
    if declared.note:
        log(f"  {declared.note}")
    results = [(materialise(ctx, entry, declared, dry, force), entry) for entry in entries]
    for path in secure_package_dirs(ctx, dry):
        log(f"  {paint('remoded', GREEN, color_on):<{len('plaintext') + 12}}{shorten(ctx, path)}")
    return summarise(ctx, results, color_on, "would apply" if dry else "applied")


def cmd_apply(ctx, dry, force):
    ctx.dry = dry
    prepare(ctx)
    if run_apply(ctx, dry, force, False):
        raise SystemExit(1)


def cmd_status(ctx):
    prepare(ctx)
    entries = plan(ctx)
    if not entries:
        log("no secrets tracked")
        return
    color_on = colors_enabled()
    declared = load_vars(ctx)
    if declared.note:
        log(f"  {declared.note}")
    results = [(inspect(ctx, entry, declared), entry) for entry in entries]
    for state, entry in results:
        show(ctx, state, entry, color_on)
    counts = {}
    for state, _entry in results:
        counts[state] = counts.get(state, 0) + 1
    log(counted(counts))
    if any(state in BLOCKING for state in counts):
        raise SystemExit(1)


def cmd_clean(ctx, dry):
    prepare(ctx)
    entries = plan(ctx)
    if not entries:
        log("no secrets tracked")
        return
    color_on = colors_enabled()
    declared = load_vars(ctx)
    results = [(unmaterialise(ctx, entry, declared, dry), entry) for entry in entries]
    if summarise(ctx, results, color_on, "would clean" if dry else "cleaned"):
        raise SystemExit(1)
