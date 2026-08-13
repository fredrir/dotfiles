import sys
from typing import Annotated

import typer

from tools.core import menu
from tools.core.console import die, out
from tools.theme import view
from tools.theme.model import Theme, list_profiles, path
from tools.theme.profiles import (
    DEFAULT_GROUP,
    THEME_KEY,
    assign,
    inventory,
    read_selection,
    unassign,
)
from tools.theme.registry import EMITTERS
from tools.theme.render import Output, ScopedOutput
from tools.theme.validate import validate

PROG = "dotfile theme"

app = typer.Typer(
    add_completion=False,
    help="Stamp the theme profiles named in profiles.dotfile into every config.",
)

MENU = (
    ("switch", "assign a profile and restamp every config"),
    ("status", "which profile each group uses"),
    ("show", "preview a profile in full"),
    ("apply", "regenerate every generated file"),
    ("check", "report what would change without writing"),
)

EVERYTHING = "everything"
WHOLE_GROUP = "the whole group"


def _owned():
    seen = []
    for emitter in EMITTERS:
        seen.extend(emitter.outputs())
    return seen


def _generate(check):
    owned = _owned()
    selection = read_selection(owned)
    themes = {}
    for name in sorted({selection.for_path(target) for target in owned}):
        themes[name] = Theme.load(name)
        validate(themes[name])
    output = Output(check=check)
    for emitter in EMITTERS:
        by_theme = {}
        for target in emitter.outputs():
            by_theme.setdefault(selection.for_path(target), []).append(target)
        for name in sorted(by_theme):
            scoped = ScopedOutput(output, [path(target) for target in by_theme[name]])
            emitter.run(themes[name], scoped)
    return selection, output.changed


def _status_rows(selection, owned):
    scopes = selection.scopes(owned)
    counts = {name: len(targets) for name, targets in selection.assignments(owned).items()}
    ordered = sorted(scopes, key=lambda name: (name != selection.default, name))
    rows = [(Theme.load(name), scopes[name], counts[name]) for name in ordered]
    rows.extend((Theme.load(name), [], 0) for name in list_profiles() if name not in scopes)
    return rows


def _interactive():
    choice = menu.pick(PROG, [name for name, _ in MENU], [text for _, text in MENU])
    if choice is None:
        return
    name = MENU[choice][0]
    if name == "switch":
        switch(profile="", scope="")
    elif name == "status":
        status()
    elif name == "show":
        show(profile="")
    elif name == "apply":
        apply()
    elif name == "check":
        check()


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
    if ctx.invoked_subcommand is not None:
        return
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        out(ctx.get_help())
        return
    _interactive()


@app.command(help="Regenerate every config from the profiles named in profiles.dotfile.")
def apply():
    _selection, changed = _generate(False)
    view.render_changes(changed, False)


@app.command(help="Report what would change without writing, and exit non-zero if anything would.")
def check():
    _selection, changed = _generate(True)
    view.render_changes(changed, True)
    if changed:
        raise typer.Exit(1)


@app.command(help="Show which profile each group resolves to, and whether anything has drifted.")
def status():
    owned = _owned()
    selection, changed = _generate(True)
    view.render_status(_status_rows(selection, owned), changed)


@app.command(help="Preview a profile: palette, roles, fonts and a sample of the terminal.")
def show(
    profile: Annotated[str, typer.Argument(help="Profile name; omit to pick one.")] = "",
):
    owned = _owned()
    selection = read_selection(owned)
    if not profile and not _interactive_terminal():
        profile = selection.default
    if not profile:
        profile = _pick_profile("preview which profile?", selection.default)
        if profile is None:
            return
    if profile not in list_profiles():
        die(PROG, f"unknown profile '{profile}' (available: {', '.join(list_profiles())})")
    theme = Theme.load(profile)
    validate(theme)
    scopes = selection.scopes(owned).get(profile, [])
    count = len(selection.assignments(owned).get(profile, []))
    view.render_show(theme, scopes, count)


@app.command(help="Assign a profile to everything, to one group, or to one package.")
def switch(
    profile: Annotated[str, typer.Argument(help="Profile name; omit to pick one.")] = "",
    scope: Annotated[
        str, typer.Argument(help="Group or group/package to assign; defaults to shared.")
    ] = "",
):
    owned = _owned()
    selection = read_selection(owned)
    groups = inventory(owned)
    everything = False
    if not profile and not _interactive_terminal():
        die(PROG, f"a profile is required (available: {', '.join(list_profiles())})")
    if scope:
        block, key, everything = _parse_scope(scope, groups)
    elif profile:
        block, key = DEFAULT_GROUP, THEME_KEY
    else:
        chosen = _pick_scope(selection, groups)
        if chosen is None:
            return
        block, key, everything = chosen
    if not profile:
        current = selection.groups.get(block, {}).get(key, selection.default)
        profile = _pick_profile(f"which profile for {_label(block, key, everything)}?", current)
        if profile is None:
            return
    if profile not in list_profiles():
        die(PROG, f"unknown profile '{profile}' (available: {', '.join(list_profiles())})")
    if everything and not _clear_overrides(selection):
        return
    assign(block, key, profile)
    out(f"  {_label(block, key, everything)} → {profile}")
    _selection, changed = _generate(False)
    view.render_changes(changed, False)
    _restart_hint(changed)


@app.command(help="Print the files this generator owns, one per line.")
def outputs(
    stageable: Annotated[
        bool, typer.Option("--stageable", help="Only the files that are safe to stage.")
    ] = False,
):
    for emitter in EMITTERS:
        if stageable and not emitter.stageable:
            continue
        for target in emitter.outputs():
            out(target)


@app.command("profiles", hidden=True)
def list_profile_names():
    for name in list_profiles():
        out(name)


def _interactive_terminal():
    return sys.stdin.isatty() and sys.stdout.isatty()


def _label(block, key, everything=False):
    if everything:
        return EVERYTHING
    return block if key == THEME_KEY else f"{block}/{key}"


def _parse_scope(scope, groups):
    scope = scope.strip("/")
    if scope == EVERYTHING:
        return DEFAULT_GROUP, THEME_KEY, True
    if scope in groups:
        return scope, THEME_KEY, False
    block, _, package = scope.rpartition("/")
    if block in groups and package in groups[block]:
        return block, package, False
    listed = ", ".join(groups)
    die(PROG, f"nothing generated is scoped to '{scope}' (groups: {listed})")
    return "", "", False


def _describe(name):
    try:
        theme = Theme.load(name)
    except SystemExit:
        return "unreadable"
    return f"{theme.name}   {'dark' if theme.dark else 'light'}"


def _pick_profile(title, default):
    names = list_profiles()
    if not names:
        die(PROG, "no profiles in theme/profiles")
    details = [_describe(name) for name in names]
    start = names.index(default) if default in names else 0
    choice = menu.pick(
        title, names, details, default=start, preview=view.picker_preview(names)
    )
    return None if choice is None else names[choice]


def _pick_scope(selection, groups):
    options = [EVERYTHING]
    details = [f"every group, now {selection.default}"]
    for group in groups:
        current = selection.groups.get(group, {}).get(THEME_KEY, selection.default)
        options.append(group)
        details.append(f"{current}   {', '.join(groups[group])}")
    choice = menu.pick("what should change?", options, details)
    if choice is None:
        return None
    if choice == 0:
        return DEFAULT_GROUP, THEME_KEY, True
    group = options[choice]
    packages = groups[group]
    if len(packages) < 2:
        return group, THEME_KEY, False
    assigned = selection.groups.get(group, {})
    current = assigned.get(THEME_KEY, selection.default)
    inner = [WHOLE_GROUP, *packages]
    hints = [f"every file in {group}, now {current}"]
    hints.extend(assigned.get(name, current) for name in packages)
    picked = menu.pick(f"all of {group}, or one package?", inner, hints)
    if picked is None:
        return None
    if picked == 0:
        return group, THEME_KEY, False
    return group, inner[picked], False


def _clear_overrides(selection):
    extra = selection.overrides()
    if not extra:
        return True
    out("  this also drops the assignments below:")
    for block, key in extra:
        out(f"      {_label(block, key)} = {selection.groups[block][key]}")
    if sys.stdin.isatty() and not typer.confirm("  drop them?", default=True):
        return False
    for block, key in extra:
        unassign(block, key)
    return True


def _restart_hint(changed):
    if any(target.startswith("linux/kde/plasma/") for target in changed):
        out("      plasma reads its own copy: systemctl --user restart plasma-plasmashell")


if __name__ == "__main__":
    app()
