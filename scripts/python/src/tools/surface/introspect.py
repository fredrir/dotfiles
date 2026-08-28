"""The command tree of a tool, read once and used for two things.

Completions and documentation both need the same answer to "what commands and
flags does this tool have", and both used to get it by hand: the zsh side had
nothing, and docs/cli was transcribed from --help. So the tree is read from the
parser itself -- Typer's click command for the python tools, `--command-dump`
for the rust ones -- and everything downstream works on this one shape.
"""

from dataclasses import dataclass

# Flags every tool has for the same reason, described once in prose.py rather
# than per tool, and skipped by the drift test.
STANDARD = ("--help", "--completions", "--version")

# What click calls a type versus what a reader should see in `--flag <THIS>`.
METAVARS = {
    "text": "TEXT",
    "str": "TEXT",
    "integer": "INTEGER",
    "int": "INTEGER",
    "float": "FLOAT",
    "path": "PATH",
    "filename": "PATH",
    "directory": "PATH",
}


@dataclass(frozen=True)
class Param:
    kind: str  # "option" or "argument"
    name: str
    opts: tuple
    secondary: tuple
    metavar: str
    help: str
    multiple: bool
    required: bool
    hidden: bool

    @property
    def flag(self):
        """The spelling the spec table and the docs key on: the long one."""
        for opt in self.opts:
            if opt.startswith("--"):
                return opt
        return self.opts[0] if self.opts else self.name

    @property
    def takes_value(self):
        return bool(self.metavar)

    @property
    def standard(self):
        return self.flag in STANDARD

    def spelling(self):
        """`-n`, `--dry-run` as a docs table cell renders it, value and all."""
        spellings = list(self.opts + self.secondary)
        if self.metavar:
            spellings[-1] = f"{spellings[-1]} <{self.metavar}>"
        return ", ".join(f"`{spelling}`" for spelling in spellings)


@dataclass(frozen=True)
class Command:
    path: tuple
    help: str
    hidden: bool
    params: tuple
    children: tuple

    @property
    def name(self):
        return self.path[-1]

    @property
    def label(self):
        return " ".join(self.path)

    def options(self):
        return tuple(param for param in self.params if param.kind == "option")

    def arguments(self):
        return tuple(param for param in self.params if param.kind == "argument")

    def visible(self):
        """Children a user is meant to find, in the order they were declared."""
        return tuple(child for child in self.children if not child.hidden)

    def walk(self, skip_hidden=True):
        """This command and every descendant, parents first."""
        if skip_hidden and self.hidden:
            return
        yield self
        for child in self.children:
            yield from child.walk(skip_hidden)

    def find(self, path):
        """The command at an absolute path, or None."""
        if path == self.path:
            return self
        for child in self.children:
            if path[: len(child.path)] == child.path:
                return child.find(path)
        return None


def one_line(text):
    return " ".join((text or "").split())


def from_typer(app, program):
    """The tree of a Typer app, as the installed command sees it."""
    from typer.main import get_command

    return _command(get_command(app), (program,))


def _command(command, path):
    registered = getattr(command, "commands", {})
    children = tuple(_command(child, path + (name,)) for name, child in registered.items())
    return Command(
        path=path,
        help=one_line(command.help or getattr(command, "short_help", "") or ""),
        hidden=bool(getattr(command, "hidden", False)),
        params=tuple(_param(param) for param in command.params),
        children=children,
    )


def _param(param):
    kind = param.param_type_name
    hidden = bool(getattr(param, "hidden", False))
    if kind == "argument":
        return Param(
            kind="argument",
            name=param.name,
            opts=(),
            secondary=(),
            metavar=_metavar(param, flag=False),
            help=one_line(getattr(param, "help", "") or ""),
            multiple=param.nargs == -1,
            required=bool(param.required),
            hidden=hidden,
        )
    flag = bool(getattr(param, "is_flag", False)) or param.type.name == "boolean"
    return Param(
        kind="option",
        name=param.name,
        opts=tuple(param.opts),
        secondary=tuple(param.secondary_opts),
        metavar="" if flag else _metavar(param, flag=False),
        help=one_line(getattr(param, "help", "") or ""),
        multiple=bool(getattr(param, "multiple", False)) or param.nargs == -1,
        required=bool(param.required),
        hidden=hidden,
    )


def _metavar(param, flag):
    if flag:
        return ""
    if getattr(param, "metavar", None):
        return param.metavar
    name = param.type.name
    if name == "boolean":
        return ""
    if name == "choice":
        return "|".join(str(choice) for choice in param.type.choices)
    return METAVARS.get(name, name.upper())


def from_click(command, program):
    """The tree of an already-built click command, as the flag callback sees it."""
    return _command(command, (program,))
