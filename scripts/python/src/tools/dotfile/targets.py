import os

from tools.dotfile.profiles import detect_platform
from tools.dotfile.state import die, trim

SCOPES = ("macos", "linux")


def platform_family():
    forced = os.environ.get("DOTFILE_PLATFORM")
    if forced:
        if forced not in SCOPES:
            die(f"DOTFILE_PLATFORM must be one of: {', '.join(SCOPES)}")
        return forced
    detected = detect_platform()
    if detected == "macos":
        return "macos"
    if detected:
        return "linux"
    return ""


def unscope(key):
    """Drop a valid macos:/linux: prefix; leave anything else untouched."""
    scope, sep, rest = key.partition(":")
    if sep and scope in SCOPES:
        return rest
    return key


def split_scope(key):
    scope, sep, rest = key.partition(":")
    if not sep:
        return "", key
    if scope not in SCOPES:
        die(f"unknown target scope '{scope}:' (expected one of: {', '.join(SCOPES)})")
    if not rest:
        die(f"missing path after '{scope}:'")
    return scope, rest


def load_targets(ctx):
    ctx.targets = {}
    if not os.path.isfile(ctx.targets_file):
        return
    scoped = {}
    with open(ctx.targets_file, encoding="utf-8") as handle:
        for line in handle.read().splitlines():
            if "=" not in line:
                continue
            key = trim(line.split("=", 1)[0])
            value = trim(line.split("=", 1)[1])
            if value.startswith("~"):
                value = ctx.home + value[1:]
            scope, path = split_scope(key)
            if scope:
                scoped.setdefault(scope, {})[path] = value
            else:
                ctx.targets[path] = value
    family = platform_family()
    if family in scoped:
        ctx.targets.update(scoped[family])


def map_dst(ctx, full, pkg, rel):
    best = ""
    for key in ctx.targets:
        if (full == key or full.startswith(key + "/")) and len(key) > len(best):
            best = key
    if not best:
        suffix = f"/{rel}" if rel else ""
        return f"{ctx.home}/.config/{pkg}{suffix}"
    if full == best:
        return ctx.targets[best]
    return f"{ctx.targets[best]}/{full[len(best) + 1 :]}"


def has_target_under(ctx, prefix):
    return any(key.startswith(prefix + "/") for key in ctx.targets)


def never_fold(ctx, path):
    protected = (
        ctx.home,
        f"{ctx.home}/.config",
        f"{ctx.home}/.local",
        f"{ctx.home}/.local/share",
        f"{ctx.home}/.local/bin",
        f"{ctx.home}/.config/systemd",
        f"{ctx.home}/.config/systemd/user",
    )
    return path in protected
