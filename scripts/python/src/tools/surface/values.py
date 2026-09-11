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


def _native(source, *args):
    import subprocess

    from tools.core.native import binary

    result = subprocess.run(
        [binary("dotfile"), "__complete", source, *args],
        capture_output=True,
        text=True,
        check=False,
        timeout=5,
    )
    if result.returncode != 0:
        return []
    from re import split

    values = []
    for line in result.stdout.splitlines():
        fields = split(r"(?<!\\):", line, maxsplit=1)
        value = fields[0].replace(r"\:", ":")
        values.append((value, fields[1]) if len(fields) == 2 else value)
    return values


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
    from tools.utils.sysinfo import hosts

    return [(name, host.role) for name, host in hosts.load_hosts().items()]


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
    "dev-packages": lambda: _native("dev-packages"),
    "dev-languages": lambda: _native("dev-languages"),
    "profiles": lambda: _native("profiles"),
    "override-groups": lambda: _native("override-groups"),
    "override-names": lambda group="": _native("override-names", group),
    "hosts": lambda: _native("hosts"),
    "packages": lambda: _native("packages"),
    "tracked": lambda: _native("tracked"),
    "recipients": lambda: _native("recipients"),
    "secrets": lambda: _native("secrets"),
    "system-files": lambda: _native("system-files"),
    "theme-profiles": lambda: _native("theme-profiles"),
    "theme-scopes": lambda: _native("theme-scopes"),
    "projects": _projects,
    "groups": _groups,
    "providers": _providers,
    "sessions": _sessions,
    "bench-hosts": _bench_hosts,
    "known-hosts": _config_hosts,
    "runs": _runs,
    "metrics": _metrics,
}
