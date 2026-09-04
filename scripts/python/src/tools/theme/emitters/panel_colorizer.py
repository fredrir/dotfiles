import glob
import os

from tools.theme.emitters._shared import hex_to_name, remap_hex
from tools.theme.model import path

PRESETS_DIR = "linux/kde/panel-colorizer/presets"


def preset_files():
    found = sorted(glob.glob(path(PRESETS_DIR, "*", "settings.json")))
    return [target for target in found if os.path.getsize(target)]


def outputs():
    return [os.path.relpath(target, path()) for target in preset_files()]


def emit(theme, out):
    mapping = hex_to_name(theme)
    for target in preset_files():
        out.edit(target, lambda text: remap_hex(theme, text, mapping))
