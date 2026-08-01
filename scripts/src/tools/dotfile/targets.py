import os

from tools.dotfile.state import trim


def load_targets(ctx):
    ctx.targets = {}
    if not os.path.isfile(ctx.targets_file):
        return
    with open(ctx.targets_file, encoding="utf-8") as handle:
        for line in handle.read().splitlines():
            if "=" not in line:
                continue
            key = trim(line.split("=", 1)[0])
            value = trim(line.split("=", 1)[1])
            if value.startswith("~"):
                value = ctx.home + value[1:]
            ctx.targets[key] = value


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
