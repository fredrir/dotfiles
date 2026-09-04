from tools.theme import yazi as yazi_theme
from tools.theme.model import Theme, list_profiles, path

MAP = "theme/maps/yazi.toml"
OUTPUT = "shared/yazi/theme.toml"
SNAPSHOT_DIR = "theme/snapshots/yazi"


def render(theme):
    with open(path(MAP), encoding="utf-8") as handle:
        template = handle.read()

    body = yazi_theme.render(theme, template)
    return f"# Yazi theme contract: {yazi_theme.YAZI_CONTRACT}\n\n# {theme.header}\n\n{body}"


def emit(theme, out):
    out.write(path(OUTPUT), render(theme))


def snapshot_outputs():
    return [f"{SNAPSHOT_DIR}/{profile}.toml" for profile in list_profiles()]


def emit_snapshots(_theme, out):
    for profile in list_profiles():
        theme = Theme.load(profile)
        out.write(path(SNAPSHOT_DIR, f"{profile}.toml"), render(theme))
