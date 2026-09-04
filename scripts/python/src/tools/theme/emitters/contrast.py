from tools.theme import contrast as contrast_report
from tools.theme.emitters.yazi import MAP as YAZI_MAP
from tools.theme.model import Theme, list_profiles, path

DIRECTORY = "theme/contrast"


def outputs():
    return [f"{DIRECTORY}/{profile}.md" for profile in list_profiles()]


def emit(_theme, out):
    with open(path(YAZI_MAP), encoding="utf-8") as handle:
        template = handle.read()
    for profile in list_profiles():
        theme = Theme.load(profile)
        out.write(path(DIRECTORY, f"{profile}.md"), contrast_report.matrix(theme, template))
