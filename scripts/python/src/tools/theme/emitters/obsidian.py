import colorsys

from tools.theme.model import load_map, path
from tools.theme.render import replace_between

DIRECTORY = "shared/obsidian/themes/Fredrir"
OUTPUT = f"{DIRECTORY}/theme.css"


def derived(theme):
    source = load_map("obsidian")["derived"]["source"]
    red, green, blue = (channel / 255 for channel in theme.rgb(theme.hex(source)))
    hue, lightness, saturation = colorsys.rgb_to_hls(red, green, blue)
    degrees = round(hue * 360)
    percent_s = round(saturation * 100)
    percent_l = round(lightness * 100)
    return {
        "accent_h": str(degrees),
        "accent_s": f"{percent_s}%",
        "accent_l": f"{percent_l}%",
        "accent_hsl": f"{degrees}, {percent_s}%, {percent_l}%",
    }


def variables(theme, derived_values):
    lines = []
    for name, value in load_map("obsidian")["variables"].items():
        if isinstance(value, str):
            lines.append(f"{name}: {theme.css(value)};")
        elif "literal" in value:
            lines.append(f"{name}: {value['literal']};")
        elif "derived" in value:
            lines.append(f"{name}: {derived_values[value['derived']]};")
        elif "rgb" in value:
            channels = ", ".join(str(channel) for channel in theme.rgb(theme.hex(value["rgb"])))
            lines.append(f"{name}: {channels};")
        else:
            channels = ", ".join(str(channel) for channel in theme.rgb(theme.hex(value["color"])))
            lines.append(f"{name}: rgba({channels}, {value['alpha']});")
    return lines


def emit(theme, out):
    lines = [f"color-scheme: {'dark' if theme.dark else 'light'};"]
    lines += variables(theme, derived(theme))
    if theme.uses_fonts("obsidian"):
        general = theme.font("general").replace("\\", "\\\\").replace('"', '\\"')
        nerd = theme.font("nerd").replace("\\", "\\\\").replace('"', '\\"')
        lines += [
            f'--font-interface-theme: "{general}", sans-serif;',
            f'--font-text-theme: "{general}", sans-serif;',
            f'--font-monospace-theme: "{nerd}", ui-monospace, monospace;',
        ]
    out.edit(path(OUTPUT), lambda text: replace_between(text, "variables", lines))
