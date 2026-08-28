import os

import pytest

from tools.dotfile.layers import (
    add_ignore,
    appended,
    base_stem,
    decision_name,
    defines,
    layer_name,
    overlay_path,
    owning_layer,
    read_patterns,
    render_pattern,
    target_layer,
)

REPO = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))

needs_repo = pytest.mark.skipif(
    not os.path.isfile(os.path.join(REPO, "shared", "vscode", "settings.json")),
    reason="needs the repo's vscode package",
)

ROOT = "/repo"


@pytest.fixture
def stack(tmp_path):
    """A shared base and a macos overlay on disk, with the layer list that names them."""
    root = tmp_path / "repo"
    base = root / "shared" / "vscode" / "settings.json"
    overlay = root / "macos" / "vscode" / "settings.macos.json"
    base.parent.mkdir(parents=True)
    overlay.parent.mkdir(parents=True)
    base.write_text('{\n\t"editor.formatOnSave": true,\n\t"[lua]": {"editor.tabSize": 4}\n}\n')
    overlay.write_text(
        '{\n\t"editor.formatOnSave": false,\n\t"shellformat.path": "/bin/shfmt"\n}\n'
    )
    return str(root), [str(base), str(overlay)]


def test_layer_name_of_the_shared_base():
    assert layer_name("shared/vscode/settings.json", ROOT) == "shared"


def test_layer_name_of_a_platform_overlay():
    assert layer_name("macos/vscode/settings.macos.json", ROOT) == "macos"


def test_layer_name_of_an_overlay_in_a_nested_group():
    assert layer_name("linux/arch/vscode/settings.arch.json", ROOT) == "arch"


def test_a_plain_json_file_is_a_base_wherever_it_lives():
    assert layer_name("macos/vscode/settings.json", ROOT) == "shared"


def test_a_tag_that_names_no_ancestor_directory_is_not_an_overlay():
    assert layer_name("macos/vscode/settings.linux.json", ROOT) == "shared"


def test_an_empty_stem_is_not_an_overlay():
    assert layer_name("macos/vscode/.macos.json", ROOT) == "shared"


def test_layer_name_takes_absolute_paths_too():
    assert layer_name(f"{ROOT}/macos/vscode/settings.macos.json", ROOT) == "macos"


def test_base_stem_strips_the_tag_and_the_suffix():
    assert base_stem("shared/vscode/settings.json", ROOT) == "settings"
    assert base_stem("macos/vscode/settings.macos.json", ROOT) == "settings"
    assert base_stem("linux/arch/vscode/keybindings.arch.json", ROOT) == "keybindings"


def test_decision_name_reads_both_forms():
    assert decision_name("shared") == "shared"
    assert decision_name("target:macos") == "macos"
    assert decision_name("target:arch") == "arch"


def test_decision_name_rejects_anything_else():
    for decision in ("", "macos", "target:", "target", "layer:macos", ":macos"):
        with pytest.raises(ValueError, match="expected 'shared' or 'target:<name>'"):
            decision_name(decision)


def test_target_layer_picks_the_base_for_shared(stack):
    root, layers = stack
    assert target_layer(layers, "shared", root) == layers[0]


def test_target_layer_picks_the_named_overlay(stack):
    root, layers = stack
    assert target_layer(layers, "target:macos", root) == layers[1]


def test_target_layer_is_none_for_a_platform_with_no_overlay_yet(stack):
    root, layers = stack
    assert target_layer(layers, "target:linux", root) is None
    assert target_layer(layers, "target:arch", root) is None


def test_target_layer_returns_an_absolute_path_for_relative_layers():
    layers = ["shared/vscode/settings.json", "macos/vscode/settings.macos.json"]
    assert target_layer(layers, "shared", ROOT) == os.path.join(ROOT, layers[0])


def test_owning_layer_searches_from_the_last_overlay_backwards(stack):
    root, layers = stack
    assert owning_layer(layers, ("editor.formatOnSave",), root) == layers[1]


def test_owning_layer_falls_back_to_the_only_layer_that_defines_the_key(stack):
    root, layers = stack
    assert owning_layer(layers, ("[lua]",), root) == layers[0]
    assert owning_layer(layers, ("shellformat.path",), root) == layers[1]


def test_owning_layer_handles_a_nested_path(stack):
    root, layers = stack
    assert owning_layer(layers, ("[lua]", "editor.tabSize"), root) == layers[0]
    assert owning_layer(layers, ("[lua]", "editor.insertSpaces"), root) is None


def test_owning_layer_is_none_when_no_layer_defines_the_key(stack):
    root, layers = stack
    assert owning_layer(layers, ("workbench.colorTheme",), root) is None


def test_owning_layer_skips_a_layer_that_is_not_on_disk_yet(stack):
    root, layers = stack
    absent = os.path.join(root, "linux", "vscode", "settings.linux.json")
    assert owning_layer([*layers, absent], ("editor.formatOnSave",), root) == layers[1]


def test_owning_layer_ignores_a_dotted_key_as_a_nesting_path(stack):
    root, layers = stack
    assert owning_layer(layers, ("editor", "formatOnSave"), root) is None


def test_defines_reads_through_comments_and_trailing_commas(tmp_path):
    layer = tmp_path / "settings.json"
    layer.write_text('{\n\t// note\n\t"a": 1,\n}\n')
    assert defines(str(layer), ("a",)) is True
    assert defines(str(layer), ("b",)) is False
    assert defines(str(tmp_path / "gone.json"), ("a",)) is False


def test_overlay_path_returns_an_overlay_that_already_exists(stack):
    root, layers = stack
    assert overlay_path(layers, "macos", root) == layers[1]


def test_overlay_path_derives_a_group_and_a_filename_for_a_new_platform(stack):
    root, layers = stack
    assert overlay_path(layers, "linux", root) == os.path.join(
        root, "linux", "vscode", "settings.linux.json"
    )


def test_overlay_naming_round_trips_through_layer_name(stack):
    root, layers = stack
    for name in ("linux", "arch", "macos"):
        assert layer_name(overlay_path(layers, name, root), root) == name


def test_overlay_path_keeps_the_base_filename(tmp_path):
    root = str(tmp_path)
    layers = ["shared/vscode/keybindings.json"]
    assert overlay_path(layers, "macos", root) == os.path.join(
        root, "macos", "vscode", "keybindings.macos.json"
    )


def test_overlay_path_needs_a_base_layer():
    with pytest.raises(ValueError, match="no layers"):
        overlay_path([], "linux", ROOT)


@needs_repo
def test_the_real_repo_layers_name_themselves():
    assert layer_name(os.path.join(REPO, "shared/vscode/settings.json"), REPO) == "shared"
    overlay = os.path.join(REPO, "macos/vscode/settings.macos.json")
    if os.path.isfile(overlay):
        assert layer_name(overlay, REPO) == "macos"
    arch = os.path.join(REPO, "linux/arch/vscode/settings.arch.json")
    if os.path.isfile(arch):
        assert layer_name(arch, REPO) == "arch"


@needs_repo
def test_the_real_shared_base_owns_a_key_it_defines():
    layers = [os.path.join(REPO, "shared/vscode/settings.json")]
    assert owning_layer(layers, ("editor.formatOnSave",), REPO) == layers[0]
    assert owning_layer(layers, ("[lua]", "editor.tabCompletion"), REPO) == layers[0]
    assert owning_layer(layers, ("nothing.like.this",), REPO) is None


def test_render_pattern_joins_nesting_with_a_slash():
    assert render_pattern(("[lua]", "editor.tabSize")) == "[lua]/editor.tabSize"


def test_render_pattern_leaves_a_dotted_key_flat():
    assert render_pattern(("cSpell.userWords",)) == "cSpell.userWords"
    assert render_pattern(("editor.formatOnSave",)) == "editor.formatOnSave"


def test_render_pattern_of_a_deep_path():
    assert render_pattern(("a", "b.c", "d")) == "a/b.c/d"


def test_render_pattern_refuses_a_key_holding_the_separator():
    with pytest.raises(ValueError, match="separates nesting levels"):
        render_pattern(("a/b",))
    with pytest.raises(ValueError, match="separates nesting levels"):
        render_pattern(("[lua]", "editor/tabSize"))


def test_render_pattern_refuses_a_key_holding_a_comment_marker():
    with pytest.raises(ValueError, match="starts a comment"):
        render_pattern(("colour#1",))


def test_render_pattern_refuses_keys_the_reader_would_reshape():
    with pytest.raises(ValueError, match="it is empty"):
        render_pattern(("",))
    with pytest.raises(ValueError, match="one line"):
        render_pattern(("a\nb",))
    with pytest.raises(ValueError, match="trims the space"):
        render_pattern((" padded",))
    with pytest.raises(ValueError, match="trims the space"):
        render_pattern(("padded\t",))


def test_render_pattern_allows_a_glob_looking_key():
    assert render_pattern(("files.associations", "*.zsh")) == "files.associations/*.zsh"


def test_read_patterns_reads_the_reader_s_format():
    text = "# a note\nignore  a.b\nignore\tc/d\n\n  ignore   e  \nnot-a-directive f\n"
    assert read_patterns(text) == ["a.b", "c/d", "e"]


def test_read_patterns_skips_a_commented_out_directive():
    assert read_patterns("# ignore a.b\nignore c.d\n") == ["c.d"]


def test_add_ignore_creates_the_file(tmp_path):
    path_to_file = str(tmp_path / "pkg" / "merge.dotfile")
    assert add_ignore(path_to_file, ("workbench.colorTheme",)) is True
    with open(path_to_file, encoding="utf-8") as handle:
        assert handle.read() == "ignore  workbench.colorTheme\n"


def test_add_ignore_appends_without_disturbing_what_is_there(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    text = "# keep whatever this machine decided\nignore  cSpell.userWords\n"
    path_to_file.write_text(text)
    assert add_ignore(str(path_to_file), ("[lua]", "editor.tabSize")) is True
    assert path_to_file.read_text() == text + "ignore  [lua]/editor.tabSize\n"


def test_add_ignore_is_idempotent(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    assert add_ignore(str(path_to_file), ("a.b",)) is True
    once = path_to_file.read_text()
    assert add_ignore(str(path_to_file), ("a.b",)) is False
    assert path_to_file.read_text() == once


def test_add_ignore_sees_a_pattern_written_in_another_column_style(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    path_to_file.write_text("ignore\t\t[lua]/editor.tabSize\n")
    assert add_ignore(str(path_to_file), ("[lua]", "editor.tabSize")) is False
    assert path_to_file.read_text() == "ignore\t\t[lua]/editor.tabSize\n"


def test_add_ignore_does_not_count_a_commented_out_pattern(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    path_to_file.write_text("# ignore  a.b\n")
    assert add_ignore(str(path_to_file), ("a.b",)) is True
    assert path_to_file.read_text() == "# ignore  a.b\nignore  a.b\n"


def test_add_ignore_matches_an_existing_column_style(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    path_to_file.write_text("ignore    first.key\n")
    add_ignore(str(path_to_file), ("second.key",))
    assert path_to_file.read_text() == "ignore    first.key\nignore    second.key\n"


def test_add_ignore_matches_a_tab_column_style(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    path_to_file.write_text("ignore\tfirst.key\n")
    add_ignore(str(path_to_file), ("second.key",))
    assert path_to_file.read_text() == "ignore\tfirst.key\nignore\tsecond.key\n"


def test_add_ignore_closes_a_file_that_lacks_a_final_newline(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    path_to_file.write_text("ignore  first.key")
    add_ignore(str(path_to_file), ("second.key",))
    assert path_to_file.read_text() == "ignore  first.key\nignore  second.key\n"


def test_add_ignore_refuses_an_unrepresentable_key(tmp_path):
    path_to_file = tmp_path / "merge.dotfile"
    with pytest.raises(ValueError, match="separates nesting levels"):
        add_ignore(str(path_to_file), ("a/b",))
    assert not path_to_file.exists()


def test_appended_keeps_crlf_line_endings():
    assert appended("ignore  a.b\r\n", "c.d") == "ignore  a.b\r\nignore  c.d\r\n"


def test_appended_starts_a_file_from_nothing():
    assert appended("", "a.b") == "ignore  a.b\n"


def test_appended_ignores_a_comment_when_choosing_the_column():
    assert appended("# ignore        spaced\n", "a.b") == "# ignore        spaced\nignore  a.b\n"
