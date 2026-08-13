import pytest

from tools.theme import registry
from tools.theme.render import replace_between, replace_ini_section, set_ini_key


def test_replace_between_swaps_the_marked_block():
    text = "before\n# theme:palette\nold\n# theme:palette:end\nafter"
    updated = replace_between(text, "palette", ["new1", "new2"])
    assert updated == "before\n# theme:palette\nnew1\nnew2\n# theme:palette:end\nafter"


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


def test_registry_marks_plasma_owned_files_unstageable():
    by_name = {emitter.name: emitter for emitter in registry.EMITTERS}
    assert not by_name["kde-colorscheme"].stageable
    assert not by_name["desktop-appletsrc"].stageable
    assert by_name["kitty"].stageable


def test_every_emitter_declares_outputs():
    for emitter in registry.EMITTERS:
        assert emitter.outputs()


def test_generate_theme_stays_a_bare_command():
    import typer.main

    from tools.theme.cli import app

    assert getattr(typer.main.get_command(app), "commands", None) is None
