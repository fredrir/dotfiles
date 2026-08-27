"""What may follow an argument, answered by the tool that owns it.

The shell asks through the hidden `__complete` command, once, only when the
cursor is actually on such a value. Every provider is called through `lines()`,
which swallows whatever it raises: a half-written config, a missing vault, a
benchmark store from another machine -- none of that may reach the terminal as
a traceback while someone is only pressing tab.
"""

import os

# Where packages live, independent of any profile: completion should offer a
# package from a group this machine does not link.
GROUP_DIRS = (
    "shared",
    "macos",
    "linux/common",
    "linux/arch",
    "linux/ubuntu",
    "linux/kde",
    "linux/hyprland",
    "linux/server",
)


def lines(name, args):
    """The values for one source, or nothing at all if it cannot be answered."""
    provider = PROVIDERS.get(name)
    if provider is None:
        return []
    try:
        return [_clean(value) for value in provider(*args) if value]
    except Exception:  # a completion never explains itself, it just stays quiet
        return []


def _clean(value):
    """`value` or `value:description`, flattened and colon-safe for _describe."""
    if isinstance(value, tuple):
        item, description = value
        return f"{_escape(item)}:{' '.join(str(description).split())}"
    return _escape(value)


def _escape(value):
    return str(value).replace(":", r"\:")


def _context():
    from tools.dotfile.state import Context
    from tools.dotfile.targets import load_targets

    ctx = Context()
    ctx.link_groups = [
        group for group in GROUP_DIRS if os.path.isdir(os.path.join(ctx.root, group))
    ]
    load_targets(ctx)
    return ctx


def _profiles():
    from tools.dotfile import profiles as profiles_module

    ctx = _context()
    relevant = set(profiles_module.list_relevant_profiles(ctx.environment_dir))
    found = []
    for name in profiles_module.list_profiles(ctx.environment_dir):
        found.append((name, "runs on this machine") if name in relevant else name)
    return found


def _override_groups():
    ctx = _context()
    found = []
    for group in GROUP_DIRS:
        if os.path.isdir(os.path.join(ctx.root, group, "overrides")):
            found.append(group)
    return found


def _override_names(group=""):
    from tools.dotfile.state import available_overrides

    ctx = _context()
    names = available_overrides(ctx, group).split() if group else []
    return [*names, ("none", "link the group without an override")]


def _hosts():
    ctx = _context()
    hosts, local = _known_hosts(ctx)
    return [(name, "this machine" if name == local else host.role) for name, host in hosts.items()]


def _known_hosts(ctx):
    from tools.utils.sysinfo import hosts as hosts_config

    path = os.path.join(ctx.root, "config/hosts.dotfile")
    hosts = hosts_config.load_hosts(path) if os.path.isfile(path) else {}
    return hosts, hosts_config.resolve(hosts=hosts)


def _packages():
    from tools.dotfile.state import each_package

    ctx = _context()
    return [(os.path.basename(name), name) for _state, _pkgdir, name in each_package(ctx)]


def _tracked():
    from tools.dotfile.state import each_package

    ctx = _context()
    return [name for _state, _pkgdir, name in each_package(ctx)]


def _recipients():
    from tools.dotfile.secret.keys import load_recipients

    return sorted(load_recipients(_context()))


def _secrets():
    from tools.dotfile.secret.vault import plan

    ctx = _context()
    found = [("vars", "the shared variables file")]
    for entry in plan(ctx):
        found.append((os.path.relpath(entry.src, ctx.root), entry.dst.replace(ctx.home, "~")))
    return found


def _system_files():
    from tools.dotfile.system import plan

    ctx = _context()
    return [(entry.dst, os.path.relpath(entry.src, ctx.root)) for entry in plan(ctx)]


def _theme_profiles():
    from tools.theme.model import list_profiles

    return list_profiles()


def _theme_scopes():
    from tools.theme.cli import _owned
    from tools.theme.profiles import inventory

    groups = inventory(_owned())
    found = [("everything", "every group and package")]
    for group, packages in groups.items():
        found.append((group, "the whole group"))
        found.extend(f"{group}/{package}" for package in packages)
    return found


def _projects():
    from tools.transcript import config

    return config.project_list()


def _groups():
    from tools.transcript import config

    return sorted(config.group_destinations())


def _providers():
    from tools.transcript import detect

    return sorted(detect.PROVIDER_MARKERS)


def _sessions(limit="25"):
    from tools.transcript import store

    found = []
    for provider, path in store.all_sessions()[: int(limit)]:
        found.append((path, f"{provider} {os.path.basename(path)}"))
    return found


def _bench_hosts():
    from tools.utils.sysinfo.bench import store

    return store.known_hosts()


def _config_hosts():
    hosts, _local = _known_hosts(_context())
    return [(name, host.role) for name, host in hosts.items()]


def _runs():
    from tools.utils.sysinfo.bench import select, store

    found = []
    for host in store.known_hosts():
        runs = store.list_runs(host, grades=select.ANY)
        found.append((host, f"{len(runs)} stored runs"))
        for epoch in select.epochs(host):
            matching = [run for run in runs if run.epoch == epoch]
            found.append((f"{host}@{epoch}", f"{len(matching)} runs on this hardware"))
    return found


def _metrics():
    from tools.utils.sysinfo.bench import select, store

    keys = set()
    for run in store.list_runs(grades=select.CLEAN):
        keys.update(metric.key for metric in run.metrics)
    return sorted(keys)


PROVIDERS = {
    "profiles": _profiles,
    "override-groups": _override_groups,
    "override-names": _override_names,
    "hosts": _hosts,
    "packages": _packages,
    "tracked": _tracked,
    "recipients": _recipients,
    "secrets": _secrets,
    "system-files": _system_files,
    "theme-profiles": _theme_profiles,
    "theme-scopes": _theme_scopes,
    "projects": _projects,
    "groups": _groups,
    "providers": _providers,
    "sessions": _sessions,
    "bench-hosts": _bench_hosts,
    "known-hosts": _config_hosts,
    "runs": _runs,
    "metrics": _metrics,
}
