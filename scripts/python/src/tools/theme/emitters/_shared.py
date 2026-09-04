import re

from tools.theme.model import list_profiles, load_map, profile_palette


def hex_to_name(theme):
    mapping = {}
    active = theme.profile
    ordered = [active] + [name for name in list_profiles() if name != active]
    for profile in ordered:
        for name, value in profile_palette(profile).items():
            mapping.setdefault(value.lstrip("#").lower(), name)
    for key, name in load_map("catppuccin")["colors"].items():
        mapping.setdefault(key.lower(), name)
    return mapping


def remap_hex(theme, text, mapping):
    pattern = re.compile(r"#([0-9a-fA-F]{8}|[0-9a-fA-F]{6})")

    def replace(match):
        token = match.group(1).lower()
        if len(token) == 6 and token in mapping:
            return theme.hex(mapping[token])
        return match.group(0)

    return pattern.sub(replace, text)
