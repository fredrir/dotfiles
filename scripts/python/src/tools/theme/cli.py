import sys
from typing import Annotated

import typer

from tools.core import menu
from tools.core.console import die, out
from tools.theme import preview
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
    help="Stamp the theme profiles named in config/profiles.dotfile into every config.",
)

MENU = (
    "sync",
    "switch",
    "status",
    "preview",
    "dry",
)

MENU_HELP = (
    "regenerate every config",
    "assign a profile to a scope",
    "resolved profiles, and drift",
    "look at a profile in full",
    "what sync would change",
)

EVERYTHING = "global"
WHOLE_GROUP = "group"


def _owned():
    seen = []
    for emitter in EMITTERS:
        seen.extend(emitter.outputs())
    return seen


def _load_all_themes():
    themes = {}
    display_names = {}
    for name in list_profiles():
        theme = Theme.load(name)
        validate(theme)
        if theme.name in display_names:
            other = display_names[theme.name]
            raise SystemExit(
                f"dotfile theme: profiles '{other}' and '{name}' share the name {theme.name!r}"
            )
        display_names[theme.name] = name
        themes[name] = theme
    return themes


def _generate(dry):
    owned = _owned()
    selection = read_selection(owned)
    themes = _load_all_themes()
    output = Output(dry=dry)
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
    owned = _owned()
    selection = read_selection(owned)
    picks = menu.cascade(PROG, _expand(selection, inventory(owned)))
    _dispatch(picks, selection, owned)


def _dispatch(picks, selection, owned, flow=""):
    if picks is None:
        return
    if picks[-1].kind == "note":
        die(PROG, picks[-1].option)
    name = flow or picks[0].option
    if name == "sync":
        sync()
    elif name == "status":
        status()
    elif name == "dry":
        dry()
    elif name == "preview":
        _render_profile(owned, selection, picks[-1].option)
    elif name == "switch":
        block, key, everything = _scope_of(picks)
        _apply_switch(selection, block, key, everything, picks[-1].option)


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
    if ctx.invoked_subcommand is not None:
        return
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        out(ctx.get_help())
        return
    _interactive()


@app.command(help="Regenerate generated theme configs")
def sync():
    _selection, changed = _generate(False)
    preview.render_changes(changed, False)


@app.command(help="Validate every profile and color mapping")
def check():
    themes = _load_all_themes()
    out(f"{len(themes)} profiles valid")


@app.command(help="Prints dry-run of sync")
def dry():
    _selection, changed = _generate(True)
    preview.render_changes(changed, True)
    if changed:
        raise typer.Exit(1)


@app.command(help="Show profile group resolvers, and drift")
def status():
    owned = _owned()
    selection, changed = _generate(True)
    preview.render_status(_status_rows(selection, owned), changed)


@app.command("preview", help="Preview a profile")
def preview_(
    profile: Annotated[str, typer.Argument(help="Profile name")] = "",
):
    owned = _owned()
    selection = read_selection(owned)
    if not profile and not _interactive_terminal():
        profile = selection.default
    if not profile:
        profile = _pick_profile("preview which profile?", selection.default)
        if profile is None:
            return
    _render_profile(owned, selection, profile)


def _render_profile(owned, selection, profile):
    if profile not in list_profiles():
        die(PROG, f"unknown profile '{profile}' (available: {', '.join(list_profiles())})")
    theme = Theme.load(profile)
    validate(theme)
    scopes = selection.scopes(owned).get(profile, [])
    count = len(selection.assignments(owned).get(profile, []))
    preview.render_show(theme, scopes, count)


@app.command(help="Assign a profile globally, to one group, or to one package.")
def switch(
    profile: Annotated[str, typer.Argument(help="Profile name; omit to pick one.")] = "",
    scope: Annotated[
        str, typer.Argument(help="Group or group/package to assign; defaults to shared.")
    ] = "",
):
    owned = _owned()
    selection = read_selection(owned)
    groups = inventory(owned)
    if not profile and not _interactive_terminal():
        die(PROG, f"a profile is required (available: {', '.join(list_profiles())})")
    if not profile and not scope:
        picks = menu.cascade(PROG, _expand(selection, groups, flow="switch"))
        _dispatch(picks, selection, owned, flow="switch")
        return
    if scope:
        block, key, everything = _parse_scope(scope, groups)
    else:
        block, key, everything = DEFAULT_GROUP, THEME_KEY, False
    if not profile:
        title = f"which profile for {_label(block, key, everything)}?"
        profile = _pick_profile(title, _current(selection, block, key))
        if profile is None:
            return
    _apply_switch(selection, block, key, everything, profile)


def _apply_switch(selection, block, key, everything, profile):
    if profile not in list_profiles():
        die(PROG, f"unknown profile '{profile}' (available: {', '.join(list_profiles())})")
    if everything and not _clear_overrides(selection):
        return
    assign(block, key, profile)
    out(f"  {_label(block, key, everything)} → {profile}")
    _selection, changed = _generate(False)
    preview.render_changes(changed, False)
    _restart_hint(changed)


@app.command(help="Print the files this generator owns")
def outputs(
    staged: Annotated[bool, typer.Option("--staged")] = False,
):
    for emitter in EMITTERS:
        if staged and not emitter.staged:
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


def _current(selection, block, key):
    return selection.groups.get(block, {}).get(key, selection.default)


def _pick_profile(title, default):
    column = _profile_column(default)
    if column.kind == "note":
        die(PROG, column.options[0])
    choice = menu.pick(title, column.options, column.details, column.index, column.preview)
    return None if choice is None else column.options[choice]


def _profile_column(default):
    names = list_profiles()
    if not names:
        return menu.Column(["no profiles in theme/profiles"], kind="note")
    return menu.Column(
        names,
        [_describe(name) for name in names],
        preview=preview.picker_preview(names),
        default=names.index(default) if default in names else 0,
        kind="profile",
    )


def _scope_column(selection, groups):
    options = [EVERYTHING]
    details = [selection.default]
    for group in groups:
        options.append(group)
        details.append(f"{_current(selection, group, THEME_KEY)}   {', '.join(groups[group])}")
    return menu.Column(options, details, kind="scope")


def _package_column(selection, group, packages):
    assigned = selection.groups.get(group, {})
    current = assigned.get(THEME_KEY, selection.default)
    details = [f"every file in {group}, now {current}"]
    details.extend(assigned.get(name, current) for name in packages)
    return menu.Column([WHOLE_GROUP, *packages], details, kind="package")


def _expand(selection, groups, flow=""):
    def expand(picks):
        if not picks:
            if flow == "switch":
                return _scope_column(selection, groups)
            return menu.Column(list(MENU), list(MENU_HELP), kind="menu")
        last = picks[-1]
        if last.kind == "menu":
            if last.option == "switch":
                return _scope_column(selection, groups)
            if last.option == "preview":
                return _profile_column(selection.default)
            return None
        if last.kind == "scope":
            if not last.index:
                return _profile_column(selection.default)
            packages = groups[last.option]
            if len(packages) < 2:
                return _profile_column(_current(selection, last.option, THEME_KEY))
            return _package_column(selection, last.option, packages)
        if last.kind == "package":
            block, key, _everything = _scope_of(picks)
            return _profile_column(_current(selection, block, key))
        return None

    return expand


def _scope_of(picks):
    scope = next((pick for pick in picks if pick.kind == "scope"), None)
    if scope is None:
        return DEFAULT_GROUP, THEME_KEY, False
    if not scope.index:
        return DEFAULT_GROUP, THEME_KEY, True
    package = next((pick for pick in picks if pick.kind == "package"), None)
    if package is None or not package.index:
        return scope.option, THEME_KEY, False
    return scope.option, package.option, False


def _clear_overrides(selection):
    extra = selection.overrides()
    if not extra:
        return True
    out("  drops the assignments below:")
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
