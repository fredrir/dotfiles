import typer

from tools.core.console import out
from tools.theme.model import Theme, list_profiles, read_active, write_active
from tools.theme.registry import EMITTERS
from tools.theme.render import Output
from tools.theme.validate import validate

app = typer.Typer(add_completion=False)


@app.command(help="Stamp the active theme profile into every config that carries colors.")
def generate(
    profile: str = typer.Option(
        None,
        "--profile",
        help="switch theme/active to this profile, then regenerate",
    ),
    list_profiles_only: bool = typer.Option(
        False,
        "--list-profiles",
        help="print the available theme profiles, marking the active one, and exit",
    ),
    list_outputs: bool = typer.Option(
        False,
        "--list-outputs",
        help="print the files this generator owns, one per line, and exit",
    ),
    stageable: bool = typer.Option(
        False,
        "--stageable",
        help="with --list-outputs, list only files that are safe to stage automatically",
    ),
    check: bool = typer.Option(
        False,
        "--check",
        help="report what would change without writing, and exit non-zero if anything would",
    ),
):
    if list_profiles_only:
        active = read_active()
        for name in list_profiles():
            out(f"{name} (active)" if name == active else name)
        return

    if list_outputs:
        for emitter in EMITTERS:
            if stageable and not emitter.stageable:
                continue
            for target in emitter.outputs():
                out(target)
        return

    theme = Theme.load(profile)
    validate(theme)

    output = Output(check=check)
    for emitter in EMITTERS:
        emitter.run(theme, output)

    switching = profile is not None and profile != read_active()
    if switching and not check:
        write_active(profile)

    if not output.changed and not switching:
        out(f"theme: already up to date ({theme.profile})")
        return

    verb = "would regenerate" if check else "regenerated"
    out(f"theme: {verb} ({theme.profile})")
    for target in output.changed:
        out(f"  {target}")
    if check:
        raise typer.Exit(1)
