"""Which tools each page in `docs/cli` documents, and where they live."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Page:
    name: str  # the file stem under docs/cli
    title: str
    programs: tuple
    source: str


PAGES = (
    Page("clipboard", "clipboard", ("cpa", "cpas", "acp"),
         "scripts/python/src/tools/utils/remote_clipboard.py"),
    Page("count", "count", ("count",), "scripts/rust/crates/count/"),
    Page("dotfile", "dotfile", ("dotfile",), "scripts/python/src/tools/dotfile/"),
    Page("flatten", "flatten", ("flatten",), "scripts/rust/crates/flatten/"),
    Page("git", "Git CLI", ("gdd", "gget", "gpp"), "scripts/rust/crates/git/"),
    Page("hwire", "hwire", ("hwire",), "scripts/rust/crates/hwire/"),
    Page("path", "path", ("path",), "scripts/rust/crates/path/"),
    Page("size", "size", ("size",), "scripts/rust/crates/size/"),
    Page("sysinfo", "sysinfo", ("sysinfo",), "scripts/python/src/tools/utils/sysinfo/"),
    Page("tardirs", "tardirs", ("tardirs",), "scripts/python/src/tools/utils/tardirs.py"),
    Page("transcript", "transcript", ("transcript",),
         "scripts/python/src/tools/transcript/"),
)

# Tools that are documented nowhere on purpose: a Hyprland keybinding target, a
# hook's helper. They still get completions.
UNDOCUMENTED = ("power-menu", "confirm-exit", "clean-copy", "update-readme-fastfetch")

RUST = ("count", "flatten", "gdd", "gget", "gpp", "hwire", "path", "size")


def page_for(program):
    for page in PAGES:
        if program in page.programs:
            return page
    return None
