import os
import subprocess

from tools.core.paths import tilde
from tools.core.process import run
from tools.dotfile.secret.apply import prepare
from tools.dotfile.secret.identity import sops_env
from tools.dotfile.secret.keys import load_recipients
from tools.dotfile.secret.variables import VARS, references, vars_file
from tools.dotfile.secret.variables import load as load_vars
from tools.dotfile.secret.vault import (
    ENC,
    FILE_MODE,
    MARKER,
    SUFFIX,
    TMPL,
    Entry,
    encrypt,
    encrypt_text,
    have_key,
    materialise,
    plan,
)
from tools.dotfile.state import canon, die, log, shorten
from tools.dotfile.system import plan as system_plan
from tools.dotfile.targets import load_targets

UNCHANGED = 200
EMPTY_MAPPING = "{}\n"


def git_add(ctx, *paths):
    subprocess.run(
        ["git", "-C", ctx.root, "add", *paths],
        stderr=subprocess.DEVNULL,
        check=False,
    )


def targets_has_line(ctx, line):
    try:
        with open(ctx.targets_file, encoding="utf-8") as handle:
            return line in handle.read().splitlines()
    except OSError:
        return False


def require_vault(ctx):
    if not load_recipients(ctx):
        die("no recipients enrolled (run: dotfile secret enroll <label>)")
    if not have_key(ctx):
        die("no age identity on this machine (run: dotfile secret init)")


def resolve_source(ctx, path):
    expanded = ctx.home + path[1:] if path.startswith("~") else path
    if not expanded.startswith("/"):
        expanded = os.path.join(os.getcwd(), expanded)
    src = canon(expanded)
    if os.path.islink(src):
        die(f"refusing to adopt a symlink: {shorten(ctx, src)}")
    if not os.path.isfile(src):
        die(f"not a file: {shorten(ctx, src)}")
    if not src.startswith(ctx.home + "/"):
        die("source must live under $HOME")
    return src


def plan_destination(ctx, src, group, pkg):
    parent = os.path.dirname(src)
    destrel = f"{group}/{pkg}/{os.path.basename(src)}{SUFFIX}"
    default = os.path.join(ctx.home, ".config", pkg)
    mapline = "" if parent == default else f"{group}/{pkg} = {tilde(parent)}"
    return destrel, mapline


def cmd_add(ctx, path, group, pkg, marker):
    if not pkg:
        die("--pkg <name> is required")
    require_vault(ctx)
    src = resolve_source(ctx, path)
    load_targets(ctx)

    destrel, mapline = plan_destination(ctx, src, group, pkg)
    dest = os.path.join(ctx.root, destrel)
    if os.path.exists(dest):
        die(f"destination exists: {destrel}")

    pkgdir = os.path.dirname(dest)
    fresh = not os.path.isdir(pkgdir)

    problem = encrypt(ctx, src, dest)
    if problem:
        die(problem)
    log(f"sealed {shorten(ctx, src)} -> {destrel}")

    marker_path = os.path.join(pkgdir, MARKER)
    if marker is not False and (marker or fresh) and not os.path.exists(marker_path):
        with open(marker_path, "w", encoding="utf-8"):
            pass
        log(f"marked {group}/{pkg} as a secret package")
        git_add(ctx, marker_path)

    os.chmod(src, FILE_MODE)
    log(f"kept   {shorten(ctx, src)} (0600)")

    if mapline and not targets_has_line(ctx, mapline):
        with open(ctx.targets_file, "a", encoding="utf-8") as handle:
            handle.write(mapline + "\n")
        log(f"mapped {mapline}")

    git_add(ctx, dest, ctx.targets_file)


def cmd_vars(ctx, unused_only):
    prepare(ctx)
    declared = load_vars(ctx)
    if declared.note:
        die(declared.note)
    if not declared.values:
        log(f"nothing declared in {VARS}")
        return
    used = {}
    for entry in plan(ctx) + system_plan(ctx):
        if entry.kind != TMPL:
            continue
        with open(entry.src, encoding="utf-8", errors="replace") as handle:
            for name in references(handle.read()):
                used.setdefault(name, []).append(shorten(ctx, entry.dst))
    names = sorted(declared.values)
    if unused_only:
        names = [name for name in names if name not in used]
    if not names:
        log("every var is referenced")
        return
    width = max(len(name) for name in names)
    for name in names:
        where = " ".join(used.get(name, [])) or "unused"
        log(f"  {name:<{width}}  {where}")
    missing = sorted(set(used) - set(declared.values))
    for name in missing:
        log(f"  {name}  referenced by {' '.join(used[name])} but not defined")
    if missing:
        raise SystemExit(1)


def matching_entries(ctx, path):
    expanded = ctx.home + path[1:] if path.startswith("~") else path
    declared_path = vars_file(ctx)
    if expanded == declared_path or path in (VARS, "vars"):
        return [Entry(declared_path, "", VARS, ENC)]
    entries = plan(ctx)
    exact = [entry for entry in entries if entry.dst == expanded or entry.src == expanded]
    if exact:
        return exact
    return [
        entry
        for entry in entries
        if entry.src.endswith("/" + path.lstrip("/")) or entry.dst.endswith("/" + path.lstrip("/"))
    ]


def cmd_edit(ctx, path):
    require_vault(ctx)
    prepare(ctx)
    found = matching_entries(ctx, path)
    if not found:
        die(f"no tracked secret matches '{path}'")
    if len(found) > 1:
        names = " ".join(entry.src[len(ctx.root) + 1 :] for entry in found)
        die(f"'{path}' matches several: {names}")

    entry = found[0]
    fresh = not os.path.exists(entry.src)
    if fresh:
        seed_vars(ctx, entry.src)
    result = run(["sops", entry.src], cwd=ctx.root, env=sops_env(ctx))
    if result.returncode == UNCHANGED:
        log(f"unchanged {shorten(ctx, entry.src)}")
        git_add(ctx, entry.src)
        return
    if result.returncode != 0:
        if fresh:
            os.remove(entry.src)
        raise SystemExit(result.returncode)

    git_add(ctx, entry.src)
    if not entry.dst:
        log(f"saved {entry.src[len(ctx.root) + 1 :]}")
        return
    state = materialise(ctx, entry, load_vars(ctx), False, True)
    log(f"{state} {shorten(ctx, entry.dst)}")


def seed_vars(ctx, path):
    if os.path.basename(path) != VARS:
        die(f"nothing to edit: {shorten(ctx, path)}")
    problem = encrypt_text(ctx, path, EMPTY_MAPPING)
    if problem:
        die(problem)
