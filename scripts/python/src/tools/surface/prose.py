from tools.surface.catalog import CATALOG

STANDARD = CATALOG["standard"]
COMMANDS = CATALOG["commands"]
FLAGS = {
    (page, flag): text for page, flags in CATALOG["flags"].items() for flag, text in flags.items()
}
