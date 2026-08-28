"""The tables in `docs/cli`, written from the parsers rather than by hand.

Only the structure is generated: which commands exist, how each flag is
spelled, what it takes as a value, and in what order they appear. The
sentences come from `prose.py`, so the pages keep the voice they were written
in while never again disagreeing with the tool about what exists.

Everything sits between markers, in the way `readme/fastfetch.py` already
updates a block of README.md, so a page can grow prose around its tables.
"""

import os

from tools.core.paths import dotfiles_root
from tools.surface import pages, prose, rust
from tools.surface.introspect import STANDARD, Param

DOCS_DIR = "docs/cli"
INDEX = "_INDEX.md"

COMMANDS_BLOCK = "cli:commands"
FLAGS_BLOCK = "cli:flags"
INDEX_BLOCK = "cli:index"

# clap gives every tool a `help` subcommand; it is noise on a reference page.
GENERATED_COMMANDS = ("help",)

HELP = Param(
    kind="option",
    name="help",
    opts=("--help",),
    secondary=(),
    metavar="",
    help="",
    multiple=False,
    required=False,
    hidden=False,
)


def roots_for(page, trees):
    """One tree per tool the page documents, or None where one is missing."""
    found = []
    for program in page.programs:
        tree = trees.get(program) if trees else None
        if tree is None and program in pages.RUST:
            tree = rust.tree(program)
        found.append((program, tree))
    return found


def commands_of(roots):
    found = []
    for _program, tree in roots:
        if tree is None:
            continue
        for command in tree.walk():
            if command.name in GENERATED_COMMANDS and len(command.path) > 1:
                continue
            if any(parent in GENERATED_COMMANDS for parent in command.path[1:-1]):
                continue
            found.append(command)
    return found


def flags_of(roots):
    """Every flag a page documents, first spelling wins, standard ones last."""
    own = {}
    standard = {}
    for command in commands_of(roots):
        for param in command.options():
            if param.hidden:
                continue
            target = standard if param.standard else own
            target.setdefault(param.flag, param)
    # click adds `--help` when it parses rather than when it is declared, so
    # the python tools have one even though no tree mentions it.
    standard.setdefault("--help", HELP)
    ordered = list(own.items())
    ordered += [(flag, standard[flag]) for flag in STANDARD if flag in standard]
    return ordered


def command_rows(page, roots):
    rows = []
    for command in commands_of(roots):
        description = prose.COMMANDS.get(command.label, "")
        rows.append([f"`{command.label}`", description])
    return rows


def flag_rows(page, roots):
    rows = []
    for flag, param in flags_of(roots):
        if param.standard:
            description = prose.STANDARD.get(flag, "")
        else:
            description = prose.FLAGS.get((page.name, flag), "")
        rows.append([param.spelling(), description])
    return rows


def table(headers, rows):
    rows = [[cell.replace("|", r"\|") for cell in row] for row in rows]
    widths = [len(header) for header in headers]
    for row in rows:
        widths = [max(width, len(cell)) for width, cell in zip(widths, row, strict=True)]
    lines = ["| " + " | ".join(h.ljust(w) for h, w in zip(headers, widths, strict=True)) + " |"]
    lines.append("| " + " | ".join("-" * width for width in widths) + " |")
    for row in rows:
        lines.append(
            "| "
            + " | ".join(cell.ljust(width) for cell, width in zip(row, widths, strict=True))
            + " |"
        )
    return "\n".join(lines)


def block(name, body):
    return f"<!-- {name}:start -->\n{body}\n<!-- {name}:end -->"


def replace_block(text, name, body):
    """The file with one marked block rewritten, or None when it has no markers."""
    start = f"<!-- {name}:start -->"
    end = f"<!-- {name}:end -->"
    if start not in text or end not in text:
        return None
    head, _, remainder = text.partition(start)
    _, _, tail = remainder.partition(end)
    return head + block(name, body) + tail


def page_text(page, roots, previous=""):
    """The page, rewriting its blocks in place, or written out when it has none."""
    rows = flag_rows(page, roots)
    commands = table(["Command", "Description"], command_rows(page, roots))
    flags = table(["Flag", "Description"], rows)
    updated = replace_block(previous, COMMANDS_BLOCK, commands) if previous else None
    updated = replace_block(updated, FLAGS_BLOCK, flags) if updated else None
    if updated:
        return updated
    body = f"# {page.title}\n\n## Commands\n\n{block(COMMANDS_BLOCK, commands)}\n"
    if rows:
        body += f"\n## Flags\n\n{block(FLAGS_BLOCK, flags)}\n"
    return body


def index_text(previous=""):
    rows = []
    for page in pages.PAGES:
        rows.append([page.name, f"[{page.name}.md](./{page.name}.md)", f"[{page.source}]"])
    body = table(["Command", "Docs", "Path"], rows)
    updated = replace_block(previous, INDEX_BLOCK, body) if previous else None
    if updated:
        return updated
    return f"# Command Line Interface (CLI)\n\n{block(INDEX_BLOCK, body)}\n"


def read(path):
    if not os.path.isfile(path):
        return ""
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def write(trees, check=False):
    """Rewrite every page. Returns the pages that changed and the tools missing."""
    root = os.path.join(str(dotfiles_root()), DOCS_DIR)
    changed = []
    missing = []
    for page in pages.PAGES:
        roots = roots_for(page, trees)
        absent = [program for program, tree in roots if tree is None]
        if absent:
            missing.extend(absent)
            continue
        path = os.path.join(root, f"{page.name}.md")
        previous = read(path)
        updated = page_text(page, roots, previous)
        if updated == previous:
            continue
        changed.append(os.path.join(DOCS_DIR, f"{page.name}.md"))
        if not check:
            _write(path, updated)
    path = os.path.join(root, INDEX)
    previous = read(path)
    updated = index_text(previous)
    if updated != previous:
        changed.append(os.path.join(DOCS_DIR, INDEX))
        if not check:
            _write(path, updated)
    return changed, missing


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def outputs():
    """Every file this generator owns, for the pre-commit hook to stage."""
    found = [os.path.join(DOCS_DIR, f"{page.name}.md") for page in pages.PAGES]
    return [*found, os.path.join(DOCS_DIR, INDEX)]
