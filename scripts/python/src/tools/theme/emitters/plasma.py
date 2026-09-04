import re

from tools.theme.emitters._shared import hex_to_name, remap_hex
from tools.theme.model import load_map, path
from tools.theme.render import replace_ini_section, set_ini_key

KDEGLOBALS = "linux/kde/plasma/kdeglobals"
DESKTOP_APPLETSRC = "linux/kde/plasma/plasma-org.kde.plasma.desktop-appletsrc"


def emit_colorscheme(theme, out):
    spec = load_map("kde")
    groups = spec["groups"]
    foregrounds = spec["foregrounds"]
    selection = spec["selection_foregrounds"]

    sections = {}
    for group, (background, alternate) in groups.items():
        overrides = selection if group == "Colors:Selection" else {}
        background_colors = (theme.kde(background), theme.kde(alternate))
        decoration = theme.readable_many(theme.data["kde"]["decoration"], background_colors, 3.0)
        body = [
            f"BackgroundAlternate={theme.rgb_csv(background_colors[1])}",
            f"BackgroundNormal={theme.rgb_csv(background_colors[0])}",
            f"DecorationFocus={theme.rgb_csv(decoration)}",
            f"DecorationHover={theme.rgb_csv(decoration)}",
        ]
        for key, role in foregrounds.items():
            selected = overrides.get(key)
            if selected:
                color = theme.kde(selected)
            else:
                floor = 3.0 if role == "inactive" else 4.5
                color = theme.readable_many(theme.data["kde"][role], background_colors, floor)
            body.append(f"{key}={theme.rgb_csv(color)}")
        sections[group] = body
    sections["WM"] = [
        f"activeBackground={theme.rgb_csv(theme.kde('wm_active_bg'))}",
        f"activeBlend={theme.rgb_csv(theme.kde('wm_active_blend'))}",
        f"activeForeground={theme.rgb_csv(theme.kde('wm_active_fg'))}",
        f"inactiveBackground={theme.rgb_csv(theme.kde('wm_inactive_bg'))}",
        f"inactiveBlend={theme.rgb_csv(theme.kde('wm_inactive_blend'))}",
        f"inactiveForeground={theme.rgb_csv(theme.kde('wm_inactive_fg'))}",
    ]
    accent = theme.rgb_csv(theme.kde("accent"))

    def transform(text):
        for header, body in sections.items():
            text = replace_ini_section(text, header, body)
        text = set_ini_key(text, "General", "AccentColor", accent)
        return set_ini_key(text, "General", "LastUsedCustomAccentColor", accent)

    out.edit(path(KDEGLOBALS), transform)


def emit_desktop_appletsrc(theme, out):
    mapping = hex_to_name(theme)
    rgb_map = {}
    for token, name in mapping.items():
        rgb_map[theme.rgb_csv("#" + token)] = theme.rgb_csv(theme.hex(name))
    pattern = re.compile(r"^([^=\[]+)=(\d{1,3},\d{1,3},\d{1,3})$")

    def transform(text):
        lines = []
        for line in remap_hex(theme, text, mapping).split("\n"):
            match = pattern.match(line)
            if match and match.group(2) in rgb_map:
                lines.append(f"{match.group(1)}={rgb_map[match.group(2)]}")
            else:
                lines.append(line)
        return "\n".join(lines)

    out.edit(path(DESKTOP_APPLETSRC), transform)
