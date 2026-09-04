import os
import re
from types import SimpleNamespace

import pytest

from tools.theme import registry, yazi
from tools.theme.emitters.nvim import emit as emit_nvim
from tools.theme.emitters.obsidian import derived as obsidian_derived
from tools.theme.emitters.obsidian import variables as obsidian_variables
from tools.theme.emitters.starship import emit as emit_starship
from tools.theme.emitters.wezterm import COLORS_DIR as WEZTERM_COLORS_DIR
from tools.theme.emitters.wezterm import _lua_key
from tools.theme.emitters.wezterm import emit as emit_wezterm
from tools.theme.emitters.wezterm import font_roles as wezterm_font_roles
from tools.theme.emitters.wezterm import outputs as wezterm_outputs
from tools.theme.emitters.yazi import OUTPUT as YAZI_THEME_FILE
from tools.theme.emitters.yazi import emit as emit_yazi
from tools.theme.emitters.yazi import render as _yazi_theme
from tools.theme.model import Theme, list_profiles, path
from tools.theme.render import replace_between, replace_ini_section, set_ini_key


class Captured:
    """An `out` that keeps the text a transform would have written."""

    def __init__(self, text):
        self.text = text

    def edit(self, _target, transform):
        self.text = transform(self.text)


class Written:
    def __init__(self):
        self.files = {}

    def write(self, target, content):
        self.files[os.path.relpath(target, path(WEZTERM_COLORS_DIR, ".."))] = content


class RootWritten:
    def __init__(self):
        self.files = {}

    def write(self, target, content):
        self.files[os.path.relpath(target, path())] = content


def nvim_config(unit="  "):
    return "\n".join(
        [
            "setup {",
            f'{unit}flavour = "mocha",',
            f"{unit}color_overrides = {{",
            f"{unit * 2}all = {{",
            f'{unit * 3}base = "#000000",',
            f"{unit * 2}}},",
            f"{unit}}},",
            f"{unit}no_italic = true,",
            "}",
        ]
    )


def test_replace_between_swaps_the_marked_block():
    text = "before\n# theme:palette\nold\n# theme:palette:end\nafter"
    updated = replace_between(text, "palette", ["new1", "new2"])
    assert updated == "before\n# theme:palette\nnew1\nnew2\n# theme:palette:end\nafter"


def test_replace_between_indents_the_block_like_its_marker():
    text = "obj {\n\t\t// theme:palette\n\t\told\n\t\t// theme:palette:end\n}"
    updated = replace_between(text, "palette", ["a", "b"])
    assert updated == "obj {\n\t\t// theme:palette\n\t\ta\n\t\tb\n\t\t// theme:palette:end\n}"


def test_replace_between_leaves_a_blank_line_blank():
    text = "  # theme:palette\n  old\n  # theme:palette:end"
    updated = replace_between(text, "palette", ["a", "", "b"])
    assert updated == "  # theme:palette\n  a\n\n  b\n  # theme:palette:end"


def test_the_nvim_palette_is_quoted_the_way_stylua_wants_it():
    out = Captured(nvim_config("\t"))
    emit_nvim(Theme.load("mocha"), out)
    assert '\n\tflavour = "mocha",\n' in out.text
    assert '\n\t\t\tbase = "#1e1e2e",\n' in out.text
    assert "\n\tno_italic = true,\n" in out.text


def test_the_nvim_flavour_follows_the_profile_lightness():
    out = Captured(nvim_config())
    emit_nvim(Theme.load("latte"), out)
    assert 'flavour = "latte",' in out.text


def test_the_nvim_palette_does_not_require_marker_comments():
    out = Captured(nvim_config())
    emit_nvim(Theme.load("mocha"), out)
    assert "theme:palette" not in out.text
    assert "no_italic = true" in out.text


def test_starship_aligns_each_run_of_entries_on_its_own():
    theme = SimpleNamespace(
        header="h",
        palette={"red": 1, "lavender": 2},
        hex=lambda name: "#000000",
        role=lambda role: "#111111",
    )
    out = Captured("# theme:palette\nold\n# theme:palette:end")
    emit_starship(theme, out)
    lines = out.text.split("\n")
    assert "red      = '#000000'" in lines
    assert "prompt_duration = '#111111'" in lines


def test_wezterm_gets_a_scheme_file_per_profile_and_a_profiles_table():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)
    expected = {f"colors/{profile}.lua" for profile in list_profiles()}
    assert expected | {"colors/profiles.lua"} <= set(out.files)


def test_wezterm_declared_outputs_match_the_files_it_writes():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)
    root = path(WEZTERM_COLORS_DIR, "..")
    declared = {os.path.relpath(path(target), root) for target in wezterm_outputs()}
    assert declared == set(out.files)


def test_a_wezterm_scheme_contains_the_canonical_profile_colors():
    out = Written()
    emit_wezterm(Theme.load("sexy-purple"), out)
    scheme = out.files["colors/sexy-purple.lua"]
    assert '  name = "Sexy Purple",' in scheme
    assert '    background = "#15152b",' in scheme
    assert '    primary = "#380d6a",' in scheme
    assert '    accent = "#101020",' in scheme
    assert '    surface = "#313247",' in scheme
    assert '    foreground = "#ffffff",' in scheme
    assert '    ansi = { "' in scheme
    assert '    brights = { "' in scheme
    assert scheme.endswith("}\n")


def test_a_wezterm_scheme_carries_only_profile_color_data():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)
    scheme = out.files["colors/mocha.lua"]
    assert "dark" not in scheme
    assert "cursor_bg" not in scheme
    assert "selection_bg" not in scheme


def test_the_wezterm_profiles_file_is_typed_inert_data_with_an_active_profile():
    out = Written()
    emit_wezterm(Theme.load("latte"), out)
    profiles = out.files["colors/profiles.lua"]
    assert "---@type DotfileColorProfiles" in profiles
    assert '  active = require "ui.colors.latte",' in profiles
    assert "  profiles = {" in profiles
    assert "function" not in profiles
    assert "config" not in profiles
    assert '    ["Catppuccin Latte"] = require("ui.colors.latte").colors,' in profiles
    assert '    ["Catppuccin Mocha"] = require("ui.colors.mocha").colors,' in profiles
    assert '    ["Sexy Purple"] = require("ui.colors.sexy-purple").colors,' in profiles
    assert '    ["Midnight Blue"] = require("ui.colors.midnight-blue").colors,' in profiles


def test_wezterm_gets_a_font_file_per_role_and_a_fonts_file():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)
    expected = {f"fonts/{role}.lua" for role in wezterm_font_roles()}
    assert expected | {"fonts/fonts.lua"} <= set(out.files)


def test_wezterm_puts_all_generated_type_definitions_in_the_types_directory():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)

    generated_types = out.files["../_types/_dotfile-theme.lua"]
    assert "---@class DotfileThemeColors" in generated_types
    assert "---@field primary string" in generated_types
    assert "---@class ColorProfile" in generated_types
    assert (
        "---@alias DotfileColorProfiles { active: ColorProfile, "
        "profiles: table<string, DotfileThemeColors> }" in generated_types
    )
    assert "---@class DotfileColorProfiles" not in generated_types
    assert "---@class FontFamily" in generated_types
    assert "---@class DotfileFonts" in generated_types

    runtime_files = [
        content for name, content in out.files.items() if name != "../_types/_dotfile-theme.lua"
    ]
    assert all(
        "---@class" not in content and "---@field" not in content for content in runtime_files
    )


def test_a_wezterm_font_role_file_is_inert_data():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)
    nerd = out.files["fonts/nerd.lua"]
    assert '  family = "Hack Nerd Font Mono",' in nerd
    assert "wezterm" not in nerd
    assert "require" not in nerd


def test_the_wezterm_fonts_file_contains_the_selected_font_settings():
    out = Written()
    theme = Theme.load("mocha")
    emit_wezterm(theme, out)
    fonts = out.files["fonts/fonts.lua"]
    assert f"  font_size = {theme.size('terminal')}," in fonts
    assert f"  interface_font_size = {theme.size('interface')}," in fonts
    assert '  nerd_family = "Hack Nerd Font Mono",' in fonts
    assert '  general_family = "Noto Sans",' in fonts


def test_generated_wezterm_runtime_files_are_inert_data():
    out = Written()
    emit_wezterm(Theme.load("mocha"), out)
    runtime_files = [
        content for name, content in out.files.items() if name != "../_types/_dotfile-theme.lua"
    ]
    assert all("function " not in content for content in runtime_files)
    assert all("config." not in content for content in runtime_files)


def test_a_wezterm_key_that_is_not_a_lua_identifier_is_bracketed():
    assert _lua_key("bright_black") == "bright_black"
    assert _lua_key("*.toml") == '["*.toml"]'
    assert _lua_key("end") == '["end"]'


@pytest.mark.parametrize("profile", list_profiles())
def test_yazi_renders_every_profile_as_a_complete_theme(profile):
    out = RootWritten()
    emit_yazi(Theme.load(profile), out)
    rendered = out.files[YAZI_THEME_FILE]
    assert f"# Generated from theme/profiles/{profile}.toml\n" in rendered
    assert "[app]" in rendered
    assert "[mgr]" in rendered
    assert "[filetype]" in rendered
    assert "[icon]" in rendered
    colors = re.findall(r'\b(?:fg|bg)\s*=\s*"([^"]+)"', rendered)
    assert colors
    assert all(re.fullmatch(r"#[0-9a-f]{6}|reset", color) for color in colors)
    assert "reversed = true" not in rendered
    assert "chord" in rendered
    assert "action" in rendered


def test_yazi_uses_semantic_colors_and_keeps_reset_literal():
    out = RootWritten()
    theme = Theme.load("mocha")
    emit_yazi(theme, out)
    rendered = out.files[YAZI_THEME_FILE]
    assert f'overall = {{ bg = "{theme.hex("canvas_bg")}" }}' in rendered
    assert f'cwd            = {{ fg = "{theme.hex("info_text_on_canvas")}" }}' in rendered
    assert (
        f'count_copied   = {{ fg = "{theme.hex("on_success_fill_on_canvas")}", '
        f'bg = "{theme.hex("success_fill_on_canvas")}" }}'
    ) in rendered
    assert (
        f'current = {{ fg = "{theme.hex("on_primary_fill_on_canvas")}", '
        f'bg = "{theme.hex("primary_fill_on_canvas")}", bold = true }}'
    ) in rendered
    assert 'find_position  = { fg = "#f9e2af", bg = "reset", italic = true }' in rendered


@pytest.mark.parametrize("profile", list_profiles())
def test_every_yazi_state_meets_its_contrast_contract(profile):
    with open(path("theme/maps/yazi.toml"), encoding="utf-8") as handle:
        pairs = yazi.contrast_pairs(Theme.load(profile), handle.read())
    assert pairs
    assert [pair for pair in pairs if not pair.passes] == []


@pytest.mark.parametrize("profile", list_profiles())
def test_yazi_full_state_snapshots(profile):
    with open(path("theme/snapshots/yazi", f"{profile}.toml"), encoding="utf-8") as handle:
        snapshot = handle.read()
    assert _yazi_theme(Theme.load(profile)) == snapshot


@pytest.mark.parametrize("profile", list_profiles())
def test_generated_contrast_matrix_covers_semantics_apps_and_raw_ansi(profile):
    with open(path("theme/contrast", f"{profile}.md"), encoding="utf-8") as handle:
        matrix = handle.read()
    assert f"# Contrast matrix: {Theme.load(profile).name}" in matrix
    assert "| semantic |" in matrix
    assert "| gtk |" in matrix
    assert "| kde |" in matrix
    assert "| obsidian |" in matrix
    assert "| yazi |" in matrix
    assert "| ansi |" in matrix
    assert "| FAIL |" not in matrix


def test_obsidian_preserves_alpha_expressions():
    theme = Theme.load("mocha")
    variables = "\n".join(obsidian_variables(theme, obsidian_derived(theme)))
    assert "--selection-background: rgba(242, 205, 205, 0.3);" in variables
    assert "--background-modifier-hover: rgba(242, 205, 205, 0.1);" in variables


def test_replace_between_requires_markers():
    with pytest.raises(SystemExit):
        replace_between("no markers", "palette", ["x"])


def test_replace_ini_section_keeps_trailing_blanks():
    text = "[General]\nold=1\n\n[Other]\nkeep=2"
    updated = replace_ini_section(text, "General", ["new=3"])
    assert updated == "[General]\nnew=3\n\n[Other]\nkeep=2"


def test_set_ini_key_updates_and_inserts_sorted():
    text = "[General]\nAlpha=1\nGamma=3"
    assert set_ini_key(text, "General", "Alpha", "9") == "[General]\nAlpha=9\nGamma=3"
    assert set_ini_key(text, "General", "Beta", "2") == "[General]\nAlpha=1\nBeta=2\nGamma=3"


def test_registry_marks_plasma_owned_files_unstaged():
    by_name = {emitter.name: emitter for emitter in registry.EMITTERS}
    assert not by_name["kde-colorscheme"].staged
    assert not by_name["desktop-appletsrc"].staged
    assert by_name["wezterm"].staged


def test_every_emitter_declares_outputs():
    for emitter in registry.EMITTERS:
        assert emitter.outputs()


def test_theme_lives_under_dotfile():
    import typer.main

    from tools.dotfile.cli import app

    commands = typer.main.get_command(app).commands
    assert "theme" in commands
    expected = {"sync", "dry", "status", "preview", "switch", "outputs", "check", "contrast"}
    assert expected <= set(commands["theme"].commands)
