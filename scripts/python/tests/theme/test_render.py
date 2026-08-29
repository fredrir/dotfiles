from types import SimpleNamespace

import pytest

from tools.theme import registry
from tools.theme.emitters import emit_nvim, emit_starship
from tools.theme.render import replace_between, replace_ini_section, set_ini_key


class Captured:
    """An `out` that keeps the text a transform would have written."""

    def __init__(self, text):
        self.text = text

    def edit(self, _target, transform):
        self.text = transform(self.text)


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


def test_the_nvim_flavour_is_quoted_the_way_stylua_wants_it():
    theme = SimpleNamespace(profile="test", data={"nvim": {"flavour": "mocha"}})
    out = Captured("\t\t\t-- theme:flavour\n\t\t\told\n\t\t\t-- theme:flavour:end")
    emit_nvim(theme, out)
    assert out.text == (
        '\t\t\t-- theme:flavour\n\t\t\tflavour = "mocha",\n\t\t\t-- theme:flavour:end'
    )


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
    expected = {"sync", "dry", "status", "preview", "switch", "outputs"}
    assert expected <= set(commands["theme"].commands)
