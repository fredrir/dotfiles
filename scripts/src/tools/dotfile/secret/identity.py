import os
import shutil
import stat

from tools.core.process import capture
from tools.dotfile.state import die


def identity_dir(ctx):
    return os.path.join(ctx.state_dir, "age")


def identity_path(ctx):
    return os.path.join(identity_dir(ctx), "keys.txt")


def stray_paths(ctx):
    config = os.path.dirname(ctx.state_dir)
    return (
        os.path.join(config, "sops", "age", "keys.txt"),
        os.path.join(ctx.home, "Library", "Application Support", "sops", "age", "keys.txt"),
    )


def have(program):
    return shutil.which(program) is not None


def mode_of(path):
    try:
        return stat.S_IMODE(os.stat(path).st_mode)
    except OSError:
        return -1


def public_key(path):
    if not os.path.isfile(path) or not have("age-keygen"):
        return ""
    result = capture(["age-keygen", "-y", path])
    if result.returncode != 0:
        return ""
    return result.stdout.strip()


def generate(ctx):
    path = identity_path(ctx)
    if os.path.exists(path):
        die(f"identity already exists: {path}")
    if not have("age-keygen"):
        die("age-keygen is not on PATH (install age)")
    os.makedirs(identity_dir(ctx), exist_ok=True)
    os.chmod(identity_dir(ctx), 0o700)
    result = capture(["age-keygen", "-o", path])
    if result.returncode != 0:
        die(f"age-keygen failed: {result.stderr.strip()}")
    os.chmod(path, 0o600)
    return path


def sops_env(ctx, identity=""):
    merged = dict(os.environ)
    merged["SOPS_AGE_KEY_FILE"] = identity or identity_path(ctx)
    return merged


def require_identity(path):
    resolved = os.path.abspath(os.path.expanduser(path))
    if not os.path.isfile(resolved):
        die(f"no such identity file: {resolved}")
    if not public_key(resolved):
        die(f"not readable as an age identity: {resolved}")
    return resolved
