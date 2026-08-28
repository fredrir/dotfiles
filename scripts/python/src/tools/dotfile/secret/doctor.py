import os
import platform

from tools.core.console import colors_enabled
from tools.core.process import capture, silent
from tools.dotfile.report import emit, heading, plural, row
from tools.dotfile.secret.canaries import all_canaries, canaries_file
from tools.dotfile.secret.identity import (
    have,
    identity_path,
    mode_of,
    public_key,
    sops_env,
    stray_paths,
)
from tools.dotfile.secret.keys import (
    is_recovery,
    keys_file,
    label_for,
    load_recipients,
    recovery_labels,
    sops_drifted,
    sops_file,
)
from tools.dotfile.secret.scan import encrypted_paths
from tools.dotfile.state import log, shorten

HOOKS = ("pre-commit", "pre-push")


def tools_row():
    missing = [name for name in ("age", "age-keygen", "sops") if not have(name)]
    if missing:
        return row("bad", "tools", "not on PATH", [(name, "") for name in missing])
    return row("ok", "tools", "age and sops present")


def identity_row(ctx):
    path = identity_path(ctx)
    if not os.path.isfile(path):
        return row(
            "bad", "identity", f"missing: {shorten(ctx, path)}", [("dotfile secret init", "")]
        )
    mode = mode_of(path)
    if mode & 0o077:
        return row(
            "bad",
            "identity",
            f"readable beyond you ({mode:04o})",
            [(f"chmod 600 {shorten(ctx, path)}", "")],
        )
    if not public_key(path):
        return row("bad", "identity", f"unreadable as an age key: {shorten(ctx, path)}")
    return row("ok", "identity", f"{shorten(ctx, path)} (0600)")


def enrolled_row(ctx, recipients):
    key = public_key(identity_path(ctx))
    if not key:
        return row("note", "enrolled", "no identity to check")
    label = label_for(recipients, key)
    if label:
        return row("ok", "enrolled", f"this machine is '{label}'")
    return row(
        "bad",
        "enrolled",
        "this machine is not a recipient yet",
        [
            ("on a machine that already decrypts, then push:", ""),
            (f"dotfile secret enroll {suggested_label()} {key}", ""),
            ("or here, with the recovery key:", ""),
            (f"dotfile secret enroll {suggested_label()} --using <recovery>", ""),
        ],
    )


def suggested_label():
    name = platform.node().split(".")[0].lower()
    cleaned = "".join(char for char in name if char.isalnum() or char in "._-")
    return cleaned or "<label>"


def recipients_row(ctx, recipients):
    if not recipients:
        return row("bad", "recipients", f"none in {shorten(ctx, keys_file(ctx))}")
    items = [(label, "recovery" if is_recovery(label) else "") for label in sorted(recipients)]
    if not recovery_labels(recipients):
        return row(
            "bad",
            "recipients",
            f"{len(recipients)} enrolled, none named recovery*",
            items + [("one lost disk loses everything encrypted", "")],
        )
    return row("ok", "recipients", f"{len(recipients)} enrolled", items)


def sops_row(ctx, recipients):
    if not recipients:
        return row("note", ".sops.yaml", "nothing to generate yet")
    if sops_drifted(ctx, recipients):
        return row(
            "bad",
            ".sops.yaml",
            "does not match config/keys.dotfile",
            [("dotfile secret sync", "")],
        )
    return row("ok", ".sops.yaml", f"matches {os.path.basename(keys_file(ctx))}")


def canaries_row(ctx):
    path = canaries_file(ctx)
    local = os.path.isfile(path)
    if local:
        mode = mode_of(path)
        if mode & 0o077:
            return row(
                "bad",
                "canaries",
                f"readable beyond you ({mode:04o})",
                [(f"chmod 600 {shorten(ctx, path)}", "")],
            )
    active, _notes = all_canaries(ctx)
    if not active:
        return row(
            "warn",
            "canaries",
            "no private values to match against",
            [("dotfile secret edit vars.enc.yaml", "")],
        )
    items = []
    if local:
        items.append(
            (shorten(ctx, path), "local only; this machine, not the others"),
        )
    return row("ok", "canaries", f"{plural(len(active), 'value')} guarded", items)


def hooks_row(ctx):
    result = capture(["git", "-C", ctx.root, "config", "core.hooksPath"])
    configured = result.stdout.strip()
    if os.path.basename(configured) != ".githooks":
        return row("bad", "hooks", "core.hooksPath is not .githooks", [("./setup.sh", "")])
    broken = [
        name for name in HOOKS if not os.access(os.path.join(ctx.root, ".githooks", name), os.X_OK)
    ]
    if broken:
        return row("bad", "hooks", "not executable", [(name, "") for name in broken])
    return row("ok", "hooks", "pre-commit and pre-push active")


def decryptable(ctx, path):
    return silent(["sops", "-d", path], cwd=ctx.root, env=sops_env(ctx)) == 0


def sealed_row(ctx):
    paths = encrypted_paths(ctx)
    if not paths:
        return row("note", "sealed", "no encrypted files yet")
    counted = plural(len(paths), "file")
    if not have("sops"):
        return row("bad", "sealed", f"{counted}, sops missing")
    locked = [path for path in paths if not decryptable(ctx, path)]
    if locked:
        return row(
            "bad",
            "sealed",
            f"{len(locked)} of {counted} will not decrypt here",
            [(path, "") for path in locked],
        )
    return row("ok", "sealed", f"{counted}, all decrypt here")


def git_config(ctx, name):
    return capture(["git", "-C", ctx.root, "config", name]).stdout.strip()


def diffs_row(ctx):
    if not os.path.isfile(os.path.join(ctx.root, ".gitattributes")):
        return row("warn", "diffs", "no .gitattributes, encrypted files diff as ciphertext")
    if git_config(ctx, "diff.sops.cachetextconv") == "true":
        return row(
            "bad",
            "diffs",
            "cachetextconv would write plaintext into .git",
            [("git config diff.sops.cachetextconv false", "")],
        )
    if not git_config(ctx, "diff.sops.textconv"):
        return row("warn", "diffs", "sops textconv not configured", [("./setup.sh", "")])
    return row("ok", "diffs", "git diff decrypts sops files locally")


def stray_finding(recipients, mine, other):
    if not other:
        return "warn", "not readable as an age key"
    if other == mine:
        return "warn", "this machine's own key, duplicated here"
    label = label_for(recipients, other)
    if not label:
        return "note", "not a recipient here, so it opens nothing in this repository"
    if is_recovery(label):
        return "warn", f"the '{label}' key, which is meant to live off-machine"
    return "warn", f"the '{label}' key; that machine's identity, on the wrong machine"


def strays_row(ctx, recipients):
    found = [path for path in stray_paths(ctx) if os.path.isfile(path)]
    if not found:
        return row("ok", "strays", "no identity outside the state directory")
    mine = public_key(identity_path(ctx))
    findings = [(path, *stray_finding(recipients, mine, public_key(path))) for path in found]
    items = [(shorten(ctx, path), note) for path, _kind, note in findings]
    if any(kind == "warn" for _path, kind, _note in findings):
        return row("warn", "strays", "another age identity on this machine", items)
    return row("note", "strays", "another age identity, unrelated to this one", items)


def cmd_doctor(ctx, show_all):
    recipients = load_recipients(ctx)
    color_on = colors_enabled()
    heading("doctor", shorten(ctx, ctx.root), color_on)

    entries = [
        tools_row(),
        identity_row(ctx),
        enrolled_row(ctx, recipients),
        recipients_row(ctx, recipients),
        sops_row(ctx, recipients),
        sealed_row(ctx),
        canaries_row(ctx),
        hooks_row(ctx),
        diffs_row(ctx),
        strays_row(ctx, recipients),
    ]
    for entry in entries:
        emit(entry, color_on)

    bad = [entry for entry in entries if entry[0] == "bad"]
    if bad:
        log("")
        log(f"{len(bad)} of {len(entries)} checks failed")
        raise SystemExit(1)
    if show_all:
        log("")
        log(f"keys   {shorten(ctx, keys_file(ctx))}")
        log(f"sops   {shorten(ctx, sops_file(ctx))}")
