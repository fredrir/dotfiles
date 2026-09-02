"""Where a value comes from, for the arguments whose type cannot say.

Nearly every constrained value in these tools is a plain `str` checked at run
time -- `--resolve skip|repo|live`, `--to <a host in config/hosts.dotfile>`,
`theme switch <profile> <scope>`. The parser therefore knows the flag exists
but not what may follow it, so the shell has to be told separately. Each entry
here is keyed by the command it belongs to, and `tests/surface` fails when a
key names a command or a parameter that no longer exists.
"""

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

RESOLUTIONS = choices("skip", "repo", "live")
GROUPS = choices(
    "shared",
    "macos",
    "linux/common",
    "linux/arch",
    "linux/ubuntu",
    "linux/kde",
    "linux/hyprland",
    "linux/server",
)
PROFILE = call("profiles", "profile")
HOST = call("hosts", "host")
PACKAGE = call("packages", "package")
OVERRIDE = pair("override-groups", "override-names", "override")
IDENTITY = files("*.txt")

# Keyed by the command as it is typed. A key naming a command that does not
# exist, or a parameter that command does not have, is a test failure.
VALUES = {
    "dotfile link": {
        "profile": PROFILE,
        "--override": OVERRIDE,
        "--resolve": RESOLUTIONS,
    },
    "dotfile sync": {
        "profile": PROFILE,
        "--override": OVERRIDE,
        "--resolve": RESOLUTIONS,
        "--to": HOST,
    },
    "dotfile doctor": {"profile": PROFILE},
    "dotfile add": {"path": files(), "--pkg": PACKAGE, "--description": NONE},
    "dotfile remove": {"path": call("tracked", "tracked path")},
    "dotfile secret scan": {"paths": files(), "--commits": NONE},
    "dotfile secret enroll": {
        "label": call("recipients", "recipient"),
        "key": NONE,
        "--using": IDENTITY,
    },
    "dotfile secret revoke": {"label": call("recipients", "recipient")},
    "dotfile secret roll": {
        "label": call("recipients", "recipient"),
        "key": NONE,
        "--using": IDENTITY,
    },
    "dotfile secret rekey": {"--using": IDENTITY},
    "dotfile secret sync": {"--using": IDENTITY},
    "dotfile secret add": {"path": files(), "--pkg": PACKAGE},
    "dotfile secret edit": {"path": call("secrets", "secret")},
    "dotfile system diff": {"path": call("system-files", "system file")},
    "dotfile system add": {"path": files(), "--pkg": PACKAGE, "--group": GROUPS},
    "dotfile theme preview": {"profile": call("theme-profiles", "theme profile")},
    "dotfile theme switch": {
        "profile": call("theme-profiles", "theme profile"),
        "scope": call("theme-scopes", "scope"),
    },
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
EXCLUSIVE = {
    "dotfile add": (
        ("--shared", "--linux", "--arch", "--ubuntu", "--kde", "--hyprland", "--server", "--macos"),
    ),
    "dotfile secret add": (
        ("--shared", "--linux", "--arch", "--ubuntu", "--kde", "--hyprland", "--macos"),
    ),
    "dotfile secret scan": (("--staged", "--commits"),),
    "dotfile sync": (("--force", "--resolve"),),
    "dotfile link": (("--force", "--resolve"),),
}


def values_for(label):
    return VALUES.get(label, {})


def exclusive_for(label):
    return EXCLUSIVE.get(label, ())
