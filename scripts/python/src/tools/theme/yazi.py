import re
import tomllib
from dataclasses import dataclass

from tools.theme import oklab

COLOR = re.compile(r'(\b(?:fg|bg)\s*=\s*")([^"]+)(")')
YAZI_CONTRACT = "26.8.15+"

SECTION_KEYS = {
    "flavor": {"dark", "light"},
    "app": {"overall"},
    "mgr": {
        "cwd",
        "find_keyword",
        "find_position",
        "symlink_target",
        "marker_copied",
        "marker_cut",
        "marker_marked",
        "marker_selected",
        "marker_symbol",
        "count_copied",
        "count_cut",
        "count_selected",
        "border_symbol",
        "border_style",
        "syntect_theme",
    },
    "indicator": {"parent", "current", "preview", "padding"},
    "tabs": {"active", "inactive", "sep_inner", "sep_outer"},
    "mode": {"normal_main", "normal_alt", "select_main", "select_alt", "unset_main", "unset_alt"},
    "status": {
        "overall",
        "sep_left",
        "sep_right",
        "progress_label",
        "progress_normal",
        "progress_error",
        "perm_type",
        "perm_read",
        "perm_write",
        "perm_exec",
        "perm_sep",
    },
    "which": {"border", "cols", "mask", "cand", "rest", "desc", "separator", "separator_style"},
    "confirm": {"border", "title", "body", "list", "btn_yes", "btn_no", "btn_labels"},
    "spot": {"border", "title", "tbl_col", "tbl_cell"},
    "notify": {"title_info", "title_warn", "title_error", "icon_info", "icon_warn", "icon_error"},
    "pick": {"border", "active", "inactive"},
    "input": {"border", "title", "value", "selected"},
    "cmp": {"border", "active", "inactive", "icon_file", "icon_folder", "icon_command"},
    "tasks": {"border", "title", "hovered"},
    "help": {"border", "chord", "action", "hovered"},
    "filetype": {"rules"},
    "icon": {
        "globs",
        "dirs",
        "files",
        "exts",
        "conds",
        "prepend_globs",
        "prepend_dirs",
        "prepend_files",
        "prepend_exts",
        "prepend_conds",
        "append_globs",
        "append_dirs",
        "append_files",
        "append_exts",
        "append_conds",
    },
}

CONTEXT = {
    "status": "panel_bg",
    "which": "panel_bg",
}


@dataclass(frozen=True)
class ContrastPair:
    state: str
    foreground: str
    background: str
    ratio: float
    floor: float
    kind: str

    @property
    def passes(self):
        return self.ratio + 1e-9 >= self.floor


def parse(template):
    try:
        return tomllib.loads(template)
    except tomllib.TOMLDecodeError as error:
        raise SystemExit(f"dotfile theme: invalid theme/maps/yazi.toml: {error}")


def _walk(value, path=()):
    if isinstance(value, dict):
        if "fg" in value or "bg" in value:
            yield ".".join(path), value
        for key, child in value.items():
            yield from _walk(child, (*path, str(key)))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _walk(child, (*path, str(index)))


def styles(template):
    return list(_walk(parse(template)))


def schema_problems(template):
    document = parse(template)
    problems = []
    for section, table in document.items():
        if section not in SECTION_KEYS:
            problems.append(f"maps/yazi.toml has unknown [{section}] for Yazi {YAZI_CONTRACT}")
            continue
        unknown = set(table) - SECTION_KEYS[section]
        if unknown:
            problems.append(
                f"maps/yazi.toml [{section}] has unknown keys for Yazi {YAZI_CONTRACT}: "
                + ", ".join(sorted(unknown))
            )
    help_keys = set(document.get("help", {}))
    obsolete = help_keys & {"on", "run", "desc", "footer"}
    if obsolete:
        problems.append(f"maps/yazi.toml [help] has obsolete keys: {', '.join(sorted(obsolete))}")
    required = {"border", "chord", "action", "hovered"}
    missing = required - help_keys
    if missing:
        problems.append(f"maps/yazi.toml [help] misses: {', '.join(sorted(missing))}")
    if "border" not in document.get("which", {}):
        problems.append("maps/yazi.toml [which] misses: border")
    for state, style in _walk(document):
        if style.get("reversed"):
            problems.append(f"maps/yazi.toml {state} uses reversed instead of an explicit pair")
    return problems


def _resolve(theme, expression):
    return theme.hex(expression)


def render(theme, template):
    problems = schema_problems(template)
    if problems:
        raise SystemExit(
            "dotfile theme: invalid Yazi map:\n" + "\n".join(f"  {p}" for p in problems)
        )

    def replace(match):
        expression = match.group(2)
        value = expression if expression == "reset" else _resolve(theme, expression)
        return f"{match.group(1)}{value}{match.group(3)}"

    rendered = COLOR.sub(replace, template)
    parse(rendered)
    return rendered


def contrast_pairs(theme, template):
    rows = []
    canvas = theme.hex("canvas_bg")
    for state, style in styles(template):
        foreground_expression = style.get("fg")
        if not foreground_expression:
            continue
        section = state.split(".", 1)[0]
        context = theme.hex(CONTEXT.get(section, "canvas_bg"))
        background_expression = style.get("bg")
        background = (
            context
            if not background_expression or background_expression == "reset"
            else _resolve(theme, background_expression)
        )
        foreground = _resolve(theme, foreground_expression)

        marker = state.startswith("mgr.marker_") and foreground == background
        graphical = marker or any(
            part in state
            for part in (
                "border",
                "progress_",
                "separator",
                "indicator.parent",
                "indicator.preview",
            )
        )
        if marker:
            background = canvas
        floor = 3.0 if graphical or "disabled" in foreground_expression else 4.5
        rows.append(
            ContrastPair(
                state,
                foreground,
                background,
                oklab.contrast_ratio(foreground, background),
                floor,
                "graphic" if graphical else "text",
            )
        )
    return rows
