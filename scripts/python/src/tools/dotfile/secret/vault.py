import os
import stat
import tempfile

from tools.core.process import capture_bytes
from tools.dotfile.secret.identity import identity_path, sops_env
from tools.dotfile.secret.keys import sops_file
from tools.dotfile.secret.variables import references, render
from tools.dotfile.state import each_package, prune_embedded_repos
from tools.dotfile.targets import map_dst, never_fold

FILE_MODE = 0o600
DIR_MODE = 0o700

MARKER = ".secret"
SYSTEM_MARKER = ".system"
SUFFIX = ".enc"
TEMPLATE = ".tmpl"

ENC = "enc"
TMPL = "tmpl"
PLAIN = "plain"

SEALED = "sealed"
CURRENT = "current"
WROTE = "wrote"
DRIFTED = "drifted"
REMODED = "remoded"
FAILED = "failed"
PLAINTEXT = "plaintext"
CLEANED = "cleaned"
ABSENT = "absent"
UNRESOLVED = "unresolved"

BLOCKING = (DRIFTED, FAILED, PLAINTEXT, UNRESOLVED)


class Entry:
    def __init__(self, src, dst, rel, kind):
        self.src = src
        self.dst = dst
        self.rel = rel
        self.kind = kind
        self.detail = ""


def is_encrypted_name(path):
    base = os.path.basename(path)
    return base.endswith(SUFFIX) or f"{SUFFIX}." in base


def is_template_name(path):
    return os.path.basename(path).endswith(TEMPLATE)


def vault_owned(path):
    return is_encrypted_name(path) or is_template_name(path)


def kind_of(path):
    if is_encrypted_name(path):
        return ENC
    if is_template_name(path):
        return TMPL
    return PLAIN


def plain_name(base):
    base = base.removesuffix(TEMPLATE)
    if base.endswith(SUFFIX):
        return base[: -len(SUFFIX)]
    return base.replace(f"{SUFFIX}.", ".", 1)


def binary_form(path):
    return os.path.basename(path).endswith(SUFFIX)


def have_key(ctx):
    return os.path.isfile(identity_path(ctx))


def decrypt(ctx, src):
    result = capture_bytes(["sops", "-d", src], env=sops_env(ctx))
    if result.returncode != 0:
        return None
    return result.stdout


def encrypt(ctx, src, dst):
    args = ["sops", "--config", sops_file(ctx), "-e"]
    if binary_form(dst):
        args += ["--input-type", "binary", "--output-type", "binary"]
    args.append(src)
    result = capture_bytes(args, env=sops_env(ctx))
    if result.returncode != 0:
        return result.stderr.decode("utf-8", errors="replace").strip() or "sops failed"
    make_private_dirs(os.path.dirname(dst))
    with open(dst, "wb") as handle:
        handle.write(result.stdout)
    return ""


def encrypt_text(ctx, dst, text):
    base = plain_name(os.path.basename(dst))
    descriptor, temp = tempfile.mkstemp(suffix=os.path.splitext(base)[1] or ".txt")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(text)
        return encrypt(ctx, temp, dst)
    finally:
        os.remove(temp)


def make_private_dirs(path):
    if not path or os.path.isdir(path):
        return
    parent = os.path.dirname(path)
    if parent and parent != path:
        make_private_dirs(parent)
    os.mkdir(path, DIR_MODE)
    os.chmod(path, DIR_MODE)


def write_private(dst, data):
    make_private_dirs(os.path.dirname(dst))
    descriptor = os.open(dst, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, FILE_MODE)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(data)
    os.chmod(dst, FILE_MODE)


def mode_of(path):
    try:
        return stat.S_IMODE(os.stat(path).st_mode)
    except OSError:
        return -1


def package_entries(ctx, pkgdir, name, whole):
    pkg = os.path.basename(pkgdir)
    found = []
    for parent, dirnames, filenames in os.walk(pkgdir):
        prune_embedded_repos(parent, dirnames)
        for base in sorted(filenames):
            if base in (MARKER, ".nolink", SYSTEM_MARKER):
                continue
            src = os.path.join(parent, base)
            kind = kind_of(src)
            if not whole and kind == PLAIN:
                continue
            rel = src[len(pkgdir) + 1 :]
            dst = map_dst(ctx, f"{name}/{rel}", pkg, rel)
            dst = os.path.join(os.path.dirname(dst), plain_name(os.path.basename(dst)))
            found.append(Entry(src, dst, rel, kind))
    return found


def secret_dirs(ctx):
    found = []
    for state, pkgdir, _name in each_package(ctx):
        if state == "secret":
            found.append(pkgdir)
    return found


def plan(ctx):
    entries = []
    for state, pkgdir, name in each_package(ctx):
        if state == "secret":
            entries.extend(package_entries(ctx, pkgdir, name, True))
        elif state == "link":
            entries.extend(package_entries(ctx, pkgdir, name, False))
    return sorted(entries, key=lambda entry: entry.dst)


def package_destination(ctx, pkgdir, name):
    pkg = os.path.basename(pkgdir)
    return map_dst(ctx, name, pkg, "")


def secure_package_dirs(ctx, dry):
    fixed = []
    for state, pkgdir, name in each_package(ctx):
        if state != "secret":
            continue
        dst = package_destination(ctx, pkgdir, name)
        if never_fold(ctx, dst) or not os.path.isdir(dst):
            continue
        if mode_of(dst) & 0o077:
            if not dry:
                os.chmod(dst, DIR_MODE)
            fixed.append(dst)
    return fixed


def produce(ctx, entry, variables):
    if entry.kind == PLAIN:
        return None, PLAINTEXT
    if entry.kind == ENC:
        if not have_key(ctx):
            return None, SEALED
        plain = decrypt(ctx, entry.src)
        return (plain, "") if plain is not None else (None, FAILED)
    with open(entry.src, "rb") as handle:
        raw = handle.read()
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        entry.detail = "template is not text"
        return None, FAILED
    used = references(text)
    if used and not variables.ok:
        return None, SEALED
    rendered, missing = render(text, variables.values)
    if missing:
        entry.detail = "unknown: " + " ".join(missing)
        return None, UNRESOLVED
    return rendered.encode("utf-8"), ""


def settle(ctx, entry, plain, dry, force):
    if os.path.islink(entry.dst):
        return DRIFTED
    if os.path.exists(entry.dst):
        with open(entry.dst, "rb") as handle:
            current = handle.read()
        if current != plain:
            if not force:
                return DRIFTED
            if not dry:
                write_private(entry.dst, plain)
            return WROTE
        if mode_of(entry.dst) != FILE_MODE:
            if not dry:
                os.chmod(entry.dst, FILE_MODE)
            return REMODED
        return CURRENT
    if not dry:
        write_private(entry.dst, plain)
    return WROTE


def materialise(ctx, entry, variables, dry, force):
    plain, problem = produce(ctx, entry, variables)
    if problem:
        return problem
    return settle(ctx, entry, plain, dry, force)


def unmaterialise(ctx, entry, variables, dry):
    if not os.path.exists(entry.dst) and not os.path.islink(entry.dst):
        return ABSENT
    plain, problem = produce(ctx, entry, variables)
    if problem:
        return problem
    if os.path.islink(entry.dst):
        return DRIFTED
    with open(entry.dst, "rb") as handle:
        if handle.read() != plain:
            return DRIFTED
    if not dry:
        os.remove(entry.dst)
    return CLEANED


def inspect(ctx, entry, variables):
    plain, problem = produce(ctx, entry, variables)
    if problem:
        return problem
    if os.path.islink(entry.dst):
        return DRIFTED
    if not os.path.exists(entry.dst):
        return ABSENT
    with open(entry.dst, "rb") as handle:
        if handle.read() != plain:
            return DRIFTED
    if mode_of(entry.dst) != FILE_MODE:
        return REMODED
    return CURRENT
