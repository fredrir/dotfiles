from tools.core.console import colors_enabled
from tools.core.process import run
from tools.dotfile.report import DIM, paint
from tools.dotfile.secret.identity import generate, have, identity_path, public_key, sops_env
from tools.dotfile.secret.keys import (
    AGE_KEY,
    LABEL,
    keys_file,
    label_for,
    load_recipients,
    save_recipients,
)
from tools.dotfile.secret.scan import encrypted_paths
from tools.dotfile.state import die, log, shorten

REWRAP_HINT = "re-wrap every encrypted file:  dotfile secret sync --rewrap"


def cmd_init(ctx):
    path = generate(ctx)
    key = public_key(path)
    log(f"created {shorten(ctx, path)} (0600)")
    log("")
    log(f"public key  {key}")
    log("")
    log("enroll it on this machine:  dotfile secret enroll <label>")
    log(f"enroll it from another:     dotfile secret enroll <label> {key}")


def cmd_enroll(ctx, label, key):
    if not LABEL.match(label):
        die(f"bad label '{label}'")
    recipients = load_recipients(ctx)
    if not key:
        key = public_key(identity_path(ctx))
        if not key:
            die("no identity on this machine (run: dotfile secret init)")
    if not AGE_KEY.match(key):
        die("not an age public key")
    owner = label_for(recipients, key)
    if owner and owner != label:
        die(f"that key is already enrolled as '{owner}'")
    if label in recipients and recipients[label] != key:
        die(f"'{label}' already has a different key (revoke it first)")
    if recipients.get(label) == key:
        log(f"{label} is already enrolled")
        return
    recipients[label] = key
    save_recipients(ctx, recipients)
    log(f"enrolled {label} in {shorten(ctx, keys_file(ctx))}")
    if encrypted_paths(ctx):
        log(REWRAP_HINT)


def cmd_revoke(ctx, label):
    recipients = load_recipients(ctx)
    if label not in recipients:
        die(f"not enrolled: {label}")
    if len(recipients) == 1:
        die("that is the only recipient; enroll another before revoking it")
    del recipients[label]
    save_recipients(ctx, recipients)
    log(f"revoked {label}")
    log("")
    log(REWRAP_HINT)
    log("re-wrapping does not un-see what that key already read")
    log("rotate the secrets themselves as well")


def cmd_keys(ctx):
    recipients = load_recipients(ctx)
    if not recipients:
        log(f"no recipients in {shorten(ctx, keys_file(ctx))}")
        return
    mine = public_key(identity_path(ctx))
    color_on = colors_enabled()
    width = max(len(label) for label in recipients)
    for label in sorted(recipients):
        here = "  this machine" if mine and recipients[label] == mine else ""
        log(f"  {label:<{width}}  {recipients[label]}" + paint(here, DIM, color_on))


def cmd_sync(ctx, rewrap):
    recipients = load_recipients(ctx)
    if not recipients:
        die("no recipients enrolled (run: dotfile secret enroll <label>)")
    changed = save_recipients(ctx, recipients)
    log("wrote .sops.yaml" if changed else ".sops.yaml already current")
    if not rewrap:
        return
    paths = encrypted_paths(ctx)
    if not paths:
        log("no encrypted files to re-wrap")
        return
    if not have("sops"):
        die("sops is not on PATH")
    failed = []
    for path in paths:
        result = run(["sops", "updatekeys", "-y", path], cwd=ctx.root, env=sops_env(ctx))
        if result.returncode != 0:
            failed.append(path)
    log(f"re-wrapped {len(paths) - len(failed)} of {len(paths)} files")
    if failed:
        for path in failed:
            log(f"  failed {path}")
        raise SystemExit(1)
