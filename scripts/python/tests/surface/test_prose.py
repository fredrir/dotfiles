"""The drift guard: docs/cli and the completion spec against the real parsers."""

import pytest

from tools.surface import entry, pages, prose, rust, spec
from tools.surface.introspect import STANDARD, Param

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


def commands_of(roots):
    found = []
    for _program, tree in roots:
        if tree is None:
            continue
        for command in tree.walk():
            if command.name in ("help",) and len(command.path) > 1:
                continue
            if any(parent in ("help",) for parent in command.path[1:-1]):
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


def _trees():
    found = entry.trees()
    for program in pages.RUST:
        tree = rust.tree(program)
        if tree is not None:
            found[program] = tree
    return found


TREES = _trees()


def _documented(page):
    roots = [(program, TREES.get(program)) for program in page.programs]
    if any(tree is None for _program, tree in roots):
        pytest.skip(f"{page.name} needs a tool this checkout has not built")
    return roots


@pytest.mark.parametrize("page", pages.PAGES, ids=lambda page: page.name)
def test_every_documented_command_has_a_description(page):
    for command in commands_of(_documented(page)):
        assert command.label in prose.COMMANDS, f"{command.label} needs a line in prose.COMMANDS"


@pytest.mark.parametrize("page", pages.PAGES, ids=lambda page: page.name)
def test_every_documented_flag_has_a_description(page):
    for flag, param in flags_of(_documented(page)):
        if param.standard:
            assert flag in prose.STANDARD
            continue
        assert (page.name, flag) in prose.FLAGS, f"{page.name} {flag} needs a line in prose.FLAGS"


def test_no_description_outlives_the_command_it_describes():
    labels = {
        command.label
        for page in pages.PAGES
        for command in commands_of([(program, TREES.get(program)) for program in page.programs])
    }
    unbuilt = {program for program in pages.RUST if TREES.get(program) is None}
    for label in prose.COMMANDS:
        if label.split(" ")[0] in unbuilt:
            continue
        assert label in labels, f"prose.COMMANDS describes {label}, which no tool has"


def test_no_description_outlives_the_flag_it_describes():
    known = set()
    for page in pages.PAGES:
        roots = [(program, TREES.get(program)) for program in page.programs]
        if any(tree is None for _program, tree in roots):
            continue
        known.update((page.name, flag) for flag, _param in flags_of(roots))
    unbuilt = {
        page.name
        for page in pages.PAGES
        if any(TREES.get(program) is None for program in page.programs)
    }
    for key in prose.FLAGS:
        if key[0] in unbuilt:
            continue
        assert key in known, f"prose.FLAGS describes {key}, which no tool has"


def test_the_standard_flags_are_described_once_for_everything():
    assert set(prose.STANDARD) == set(STANDARD)


def test_every_value_source_names_a_parameter_that_exists():
    labels = {}
    for tree in TREES.values():
        for command in tree.walk(skip_hidden=False):
            labels[command.label] = command
    for label, sources in spec.VALUES.items():
        command = labels.get(label)
        assert command is not None, f"spec.VALUES has {label}, which is not a command"
        keys = {param.flag for param in command.options()}
        keys |= {param.name for param in command.arguments()}
        for key in sources:
            assert key in keys, f"{label} has no {key}"


def test_every_exclusive_group_names_flags_that_exist():
    labels = {}
    for tree in TREES.values():
        for command in tree.walk(skip_hidden=False):
            labels[command.label] = command
    for label, groups in spec.EXCLUSIVE.items():
        command = labels.get(label)
        assert command is not None, f"spec.EXCLUSIVE has {label}, which is not a command"
        spellings = {param.flag for param in command.options()}
        for group in groups:
            for flag in group:
                assert flag in spellings, f"{label} has no {flag}"


def test_every_installed_command_is_documented_or_deliberately_not():
    covered = {program for page in pages.PAGES for program in page.programs}
    for program in entry.programs():
        assert program in covered or program in pages.UNDOCUMENTED
