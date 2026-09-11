"""Dynamic completion values for the remaining Python commands."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Source:
    kind: str  # choices, call, pair, files, dirs, commas or none
    values: tuple
    tag: str
    pattern: str = ""
    loader: object = None

    def options(self):
        """The literal values. A loader defers the import until the tool that
        owns the constant is the one being completed -- `dotfile` must not pay
        for sysinfo's collector just to describe `--tier`."""
        return tuple(self.loader()) if self.loader else self.values


def choices(*values):
    return Source(kind="choices", values=tuple(values), tag="")


def call(name, tag):
    """Values the tool itself lists, through its hidden `__complete`."""
    return Source(kind="call", values=(name,), tag=tag)


def pair(groups, names, tag):
    """A `<group>=<name>` value, completed one half at a time."""
    return Source(kind="pair", values=(groups, names), tag=tag)


def commas(tag, loader):
    return Source(kind="commas", values=(), tag=tag, loader=loader)


def deferred(loader):
    """Choices that live as a constant in another tool's module."""
    return Source(kind="choices", values=(), tag="", loader=loader)


def _tiers():
    from tools.utils.sysinfo.bench.record import TIERS

    return TIERS


def _families():
    from tools.utils.sysinfo.bench.runner import FAMILIES

    return FAMILIES


def files(pattern=""):
    return Source(kind="files", values=(), tag="", pattern=pattern)


def dirs():
    return Source(kind="dirs", values=(), tag="")


NONE = Source(kind="none", values=(), tag="")

# Keyed by the command as it is typed. A key naming a command that does not
# exist, or a parameter that command does not have, is a test failure.
VALUES = {
    "sysinfo bench run": {
        "--tier": deferred(_tiers),
        "--only": commas("family", _families),
        "--host": call("known-hosts", "host"),
        "--workdir": dirs(),
        "--note": NONE,
        "--tag": NONE,
    },
    "sysinfo bench show": {"target": call("runs", "run")},
    "sysinfo bench list": {"--host": call("bench-hosts", "host")},
    "sysinfo bench health": {"--host": call("bench-hosts", "host")},
    "sysinfo bench prune": {"--host": call("bench-hosts", "host")},
    "sysinfo bench compare": {"left": call("runs", "run"), "right": call("runs", "run")},
    "sysinfo bench trend": {"target": call("runs", "run"), "metric": call("metrics", "metric")},
    "sysinfo bench baseline": {
        "action": choices("set", "clear", "show"),
        "target": call("runs", "run"),
    },
    "transcript capture": {"--provider": call("providers", "provider"), "--fallback": files()},
    "transcript import": {"target": call("sessions", "session")},
    "transcript add": {"path": dirs(), "--group": call("groups", "group"), "--name": NONE},
    "transcript rm": {"target": call("projects", "project")},
    "tardirs": {"archive": files("*.(tar|tgz|tbz2|txz|tar.gz|tar.bz2|tar.xz)")},
}

# Flags that rule each other out, so completing one drops the rest.
EXCLUSIVE = {}


def values_for(label):
    return VALUES.get(label, {})


def exclusive_for(label):
    return EXCLUSIVE.get(label, ())
