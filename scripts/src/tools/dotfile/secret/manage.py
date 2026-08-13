import subprocess

from tools.core.console import colors_enabled
from tools.core.process import run
from tools.dotfile.report import DIM, paint, plural
from tools.dotfile.secret.doctor import suggested_label
from tools.dotfile.secret.identity import (
    generate,
    have,
    identity_path,
    public_key,
    require_identity,
    sops_env,
)
from tools.dotfile.secret.keys import (
    AGE_KEY,
    LABEL,
    keys_file,
    label_for,
    load_recipients,
    save_recipients,
    sops_file,
)
from tools.dotfile.secret.scan import encrypted_paths
from tools.dotfile.state import die, log, shorten


def cmd_init(ctx):
    path = generate(ctx)
    key = public_key(path)
    log(f"created {shorten(ctx, path)} (0600)")
    log("")
    log(f"public key  {key}")
    log("")
    label = suggested_label()
    log("enrolling needs a key that already decrypts. run this on a machine")
    log("that has one, then push:")
    log("")
    log(f"    dotfile secret enroll {label} {key}")
    log("")
    log("no machine reachable? the recovery key counts. put it somewhere")
    log("readable and run it here instead:")
    log("")
    log(f"    dotfile secret enroll {label} --using /path/to/recovery.txt")
    log("")
    log("then:  git commit -am 'enrol' && git push && dotfile secret apply")


def cmd_enroll(ctx, label, key, using=""):
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
    identity = require_identity(using) if using else ""
    recipients[label] = key
    save_recipients(ctx, recipients)
    log(f"enrolled {label} in {shorten(ctx, keys_file(ctx))}")
    if rewrap_all(ctx, identity):
        raise SystemExit(1)
    stage(ctx)
    log("staged; commit and push so the new machine can read them")


def cmd_revoke(ctx, label):
    recipients = load_recipients(ctx)
    if label not in recipients:
        die(f"not enrolled: {label}")
    if len(recipients) == 1:
        die("that is the only recipient; enroll another before revoking it")
    del recipients[label]
    save_recipients(ctx, recipients)
    log(f"revoked {label}")
    failed = rewrap_all(ctx)
    stage(ctx)
    log("")
    log("re-wrapping does not un-see what that key already read, and an older")
    log("clone still opens with it; rotate the secrets themselves as well")
    if failed:
        raise SystemExit(1)


def stage(ctx):
    paths = [keys_file(ctx), sops_file(ctx), *encrypted_paths(ctx)]
    subprocess.run(
        ["git", "-C", ctx.root, "add", *paths],
        stderr=subprocess.DEVNULL,
        check=False,
    )


def rewrap_all(ctx, identity=""):
    paths = encrypted_paths(ctx)
    if not paths:
        log("no encrypted files tracked, so nothing was re-wrapped")
        return False
    if not have("sops"):
        log("  sops is not on PATH; re-wrap manually with: dotfile secret sync --rewrap")
        return True
    failed = []
    for path in paths:
        env = sops_env(ctx, identity)
        if run(["sops", "updatekeys", "-y", path], cwd=ctx.root, env=env).returncode != 0:
            failed.append(path)
    log(f"re-wrapped {len(paths) - len(failed)} of {plural(len(paths), 'file')}")
    for path in failed:
        log(f"  failed {path}")
    return bool(failed)


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


def cmd_sync(ctx, rewrap, using=""):
    recipients = load_recipients(ctx)
    if not recipients:
        die("no recipients enrolled (run: dotfile secret enroll <label>)")
    changed = save_recipients(ctx, recipients)
    log("wrote .sops.yaml" if changed else ".sops.yaml already current")
    if not rewrap:
        return
    if not encrypted_paths(ctx):
        log("no encrypted files to re-wrap")
        return
    if rewrap_all(ctx, require_identity(using) if using else ""):
        raise SystemExit(1)
    stage(ctx)
