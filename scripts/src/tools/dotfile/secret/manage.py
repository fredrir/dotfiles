import os
import shutil
import subprocess

from tools.core.console import colors_enabled
from tools.core.process import run, silent
from tools.dotfile.report import DIM, paint, plural
from tools.dotfile.secret.doctor import suggested_label
from tools.dotfile.secret.identity import (
    generate,
    generate_at,
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
    sops_document,
    sops_file,
    write_if_changed,
)
from tools.dotfile.secret.scan import encrypted_paths
from tools.dotfile.secret.vault import FILE_MODE
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
    failed = rewrap_all(ctx, rotate=True)
    stage(ctx)
    caveat()
    if failed:
        raise SystemExit(1)


def caveat():
    log("")
    log("this changes what the old key can open from now on; an older clone")
    log("still holds ciphertext it can still read, and the secrets themselves")
    log("are unchanged, so rotate anything that key actually protected")


def cmd_rekey(ctx, using):
    if not load_recipients(ctx):
        die("no recipients enrolled")
    failed = rotate_all(ctx, require_identity(using) if using else "")
    stage(ctx)
    caveat()
    if failed:
        raise SystemExit(1)


def cmd_roll(ctx, label, key, using):
    recipients = load_recipients(ctx)
    if label not in recipients:
        die(f"not enrolled: {label} (use: dotfile secret enroll {label} <key>)")
    identity = require_identity(using) if using else ""
    if key:
        roll_recipient(ctx, recipients, label, key, identity)
    else:
        roll_this_machine(ctx, recipients, label, identity)


def roll_recipient(ctx, recipients, label, key, identity):
    if not AGE_KEY.match(key):
        die("not an age public key")
    if key == recipients[label]:
        log(f"{label} already has that key")
        return
    owner = label_for(recipients, key)
    if owner:
        die(f"that key is already enrolled as '{owner}'")
    recipients[label] = key
    save_recipients(ctx, recipients)
    log(f"rolled {label} to a new key")
    failed = rewrap_all(ctx, identity, rotate=True)
    stage(ctx)
    caveat()
    if failed:
        raise SystemExit(1)


def roll_this_machine(ctx, recipients, label, identity):
    mine = public_key(identity_path(ctx))
    if not mine:
        die("no identity on this machine (run: dotfile secret init)")
    if recipients[label] != mine:
        die(f"'{label}' is not this machine's key; pass the new public key as an argument")

    paths = encrypted_paths(ctx)
    unreadable = [path for path in paths if not readable(ctx, path, identity)]
    if unreadable:
        die("this machine cannot read " + " ".join(unreadable) + "; fix that before rolling")

    fresh = identity_path(ctx) + ".new"
    kept = identity_path(ctx) + ".previous"
    for path in (fresh, kept):
        if os.path.exists(path):
            die(f"leftover from an interrupted roll: {path}")

    original = dict(recipients)
    generate_at(fresh)
    newpub = public_key(fresh)
    try:
        widened = dict(recipients)
        widened[f"{label}.rolling"] = newpub
        write_if_changed(sops_file(ctx), sops_document(widened))
        if rewrap_all(ctx, identity):
            die("could not re-wrap with the current key; nothing was installed")
        if any(not readable(ctx, path, fresh) for path in paths):
            die("the new key cannot read the files; nothing was installed")
    except BaseException:
        save_recipients(ctx, original)
        os.remove(fresh)
        raise

    shutil.copy2(identity_path(ctx), kept)
    os.replace(fresh, identity_path(ctx))
    os.chmod(identity_path(ctx), FILE_MODE)
    log(f"installed a new identity at {shorten(ctx, identity_path(ctx))}")

    recipients[label] = newpub
    save_recipients(ctx, recipients)
    failed = rewrap_all(ctx, rotate=True)
    if failed or any(not readable(ctx, path) for path in paths):
        log(f"the previous identity is kept at {shorten(ctx, kept)}")
        failed = True
    else:
        os.remove(kept)
    stage(ctx)
    log(f"rolled {label}")
    caveat()
    if failed:
        raise SystemExit(1)


def stage(ctx):
    paths = [keys_file(ctx), sops_file(ctx), *encrypted_paths(ctx)]
    subprocess.run(
        ["git", "-C", ctx.root, "add", *paths],
        stderr=subprocess.DEVNULL,
        check=False,
    )


def sops_on(ctx, args, identity=""):
    return run(["sops", *args], cwd=ctx.root, env=sops_env(ctx, identity))


def readable(ctx, path, identity=""):
    return silent(["sops", "-d", path], cwd=ctx.root, env=sops_env(ctx, identity)) == 0


def rewrap_all(ctx, identity="", rotate=False):
    paths = encrypted_paths(ctx)
    if not paths:
        log("no encrypted files tracked, so nothing was re-wrapped")
        return False
    if not have("sops"):
        log("  sops is not on PATH; re-wrap manually with: dotfile secret sync --rewrap")
        return True
    failed = []
    for path in paths:
        if sops_on(ctx, ["updatekeys", "-y", path], identity).returncode != 0:
            failed.append(path)
            continue
        if rotate and sops_on(ctx, ["-r", "-i", path], identity).returncode != 0:
            failed.append(path)
    verb = "re-wrapped and gave a new data key to" if rotate else "re-wrapped"
    log(f"{verb} {len(paths) - len(failed)} of {plural(len(paths), 'file')}")
    for path in failed:
        log(f"  failed {path}")
    return bool(failed)


def rotate_all(ctx, identity=""):
    paths = encrypted_paths(ctx)
    if not paths:
        log("no encrypted files tracked")
        return False
    if not have("sops"):
        die("sops is not on PATH")
    failed = [p for p in paths if sops_on(ctx, ["-r", "-i", p], identity).returncode != 0]
    log(f"new data key on {len(paths) - len(failed)} of {plural(len(paths), 'file')}")
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
