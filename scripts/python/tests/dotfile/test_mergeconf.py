import pytest
import typer

from tools.dotfile import mergeconf


def write_conf(directory, text):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / mergeconf.NAME).write_text(text)
    return str(directory)


def test_load_ignores_reads_directives_and_skips_comments(tmp_path):
    pkgdir = write_conf(
        tmp_path / "shared" / "vscode",
        "# keep the machine's own theme\nignore  workbench.colorTheme\n\nignore cSpell.*\n",
    )
    assert mergeconf.load_ignores([pkgdir]) == ["workbench.colorTheme", "cSpell.*"]


def test_load_ignores_unions_every_layer_of_the_package(tmp_path):
    shared = write_conf(tmp_path / "shared" / "vscode", "ignore cSpell.*\nignore [lua]/*\n")
    macos = write_conf(tmp_path / "macos" / "vscode", "ignore cSpell.*\nignore shellformat.path\n")
    assert mergeconf.load_ignores([shared, macos]) == [
        "cSpell.*",
        "[lua]/*",
        "shellformat.path",
    ]


def test_load_ignores_is_empty_without_a_file(tmp_path):
    (tmp_path / "vscode").mkdir()
    assert mergeconf.load_ignores([str(tmp_path / "vscode")]) == []


def test_load_ignores_rejects_an_unknown_directive(tmp_path):
    pkgdir = write_conf(tmp_path / "vscode", "keep workbench.colorTheme\n")
    with pytest.raises(typer.Exit):
        mergeconf.load_ignores([pkgdir])


def test_load_ignores_rejects_a_directive_without_a_pattern(tmp_path):
    pkgdir = write_conf(tmp_path / "vscode", "ignore\n")
    with pytest.raises(typer.Exit):
        mergeconf.load_ignores([pkgdir])


def test_a_dot_is_part_of_a_flat_key_not_a_separator():
    patterns = ["editor.formatOnSave"]
    assert mergeconf.matches(("editor.formatOnSave",), patterns)
    assert not mergeconf.matches(("editor", "formatOnSave"), patterns)


def test_a_glob_matches_within_one_key_segment():
    patterns = ["cSpell.*"]
    assert mergeconf.matches(("cSpell.userWords",), patterns)
    assert mergeconf.matches(("cSpell.language",), patterns)
    assert not mergeconf.matches(("editor.tabSize",), patterns)


def test_slash_nests_and_brackets_are_literal():
    patterns = ["[lua]/editor.tabSize"]
    assert mergeconf.matches(("[lua]", "editor.tabSize"), patterns)
    assert not mergeconf.matches(("[python]", "editor.tabSize"), patterns)
    assert not mergeconf.matches(("editor.tabSize",), patterns)
    # fnmatch would read [lua] as "one of l, u, a"
    assert not mergeconf.matches(("l", "editor.tabSize"), patterns)
    assert not mergeconf.matches(("u",), ["[lua]"])


def test_a_pattern_covers_everything_under_the_key_it_names():
    assert mergeconf.matches(("[lua]", "editor.tabSize"), ["[lua]/*"])
    assert mergeconf.matches(("[lua]", "a", "b"), ["[lua]/*"])
    assert mergeconf.matches(("[lua]", "editor.tabSize"), ["[lua]"])
    assert not mergeconf.matches(("[lua]",), ["[lua]/editor.tabSize"])


def test_matching_is_case_sensitive_and_needs_a_pattern():
    assert not mergeconf.matches(("cspell.userWords",), ["cSpell.*"])
    assert not mergeconf.matches(("anything",), [])
