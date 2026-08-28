import difflib
import os

import pytest

from tools.dotfile import jsonc
from tools.dotfile.adopt import apply_remove, apply_set, remove_key, set_key

REPO = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
REAL = os.path.join(REPO, "shared", "vscode", "settings.json")

needs_repo_file = pytest.mark.skipif(
    not os.path.isfile(REAL), reason="needs shared/vscode/settings.json"
)

COMMENTED = (
    "{\n"
    "\t// git\n"
    '\t"git.autofetch": true, // trailing\n'
    "\t/* a block */\n"
    '\t"[lua]": {\n'
    '\t\t"editor.tabSize": 4\n'
    "\t},\n"
    '\t"files.associations": {\n'
    '\t\t"*.zsh": "shellscript"\n'
    "\t}\n"
    "}\n"
)


def parse(text):
    """The one place these tests lean on the JSONC reader."""
    return jsonc.loads(text)


def changed(old, new):
    """The lines `new` dropped and the lines it gained, in order.

    These tests are about bytes, against a file that is hand-authored and
    edited often, so they state the shape of the edit rather than naming the
    settings that happen to sit around it today.
    """
    diff = list(difflib.ndiff(old.splitlines(), new.splitlines()))
    dropped = [line[2:] for line in diff if line.startswith("- ")]
    gained = [line[2:] for line in diff if line.startswith("+ ")]
    return dropped, gained


def real_text():
    with open(REAL, encoding="utf-8", newline="") as handle:
        return handle.read()


def copy_to(tmp_path, text):
    target = tmp_path / "settings.json"
    target.write_text(text, encoding="utf-8", newline="")
    return str(target)


def read_back(path_to_file):
    with open(path_to_file, encoding="utf-8", newline="") as handle:
        return handle.read()


def test_replace_touches_only_the_value():
    out = apply_set(COMMENTED, ("git.autofetch",), False)
    assert out == COMMENTED.replace('"git.autofetch": true', '"git.autofetch": false')
    assert "// git" in out
    assert "// trailing" in out
    assert "/* a block */" in out


def test_replace_keeps_the_comment_that_trails_the_member():
    text = '{\n\t"a": 1 // note\n}\n'
    assert apply_set(text, ("a",), 2) == '{\n\t"a": 2 // note\n}\n'


def test_replace_a_scalar_with_a_structure_uses_the_file_indent():
    text = '{\n\t"a": 1,\n\t"b": 2\n}\n'
    out = apply_set(text, ("a",), {"c": [1, 2]})
    assert out == '{\n\t"a": {\n\t\t"c": [\n\t\t\t1,\n\t\t\t2\n\t\t]\n\t},\n\t"b": 2\n}\n'
    assert parse(out) == {"a": {"c": [1, 2]}, "b": 2}


def test_insert_commas_the_previous_member():
    text = '{\n\t"a": 1\n}\n'
    assert apply_set(text, ("b",), 2) == '{\n\t"a": 1,\n\t"b": 2\n}\n'


def test_insert_after_a_member_that_carries_a_trailing_comment():
    text = '{\n\t"a": 1 // note\n}\n'
    assert apply_set(text, ("b",), 2) == '{\n\t"a": 1, // note\n\t"b": 2\n}\n'


def test_insert_after_an_existing_trailing_comma_adds_no_second_comma():
    text = '{\n\t"a": 1,\n}\n'
    assert apply_set(text, ("b",), 2) == '{\n\t"a": 1,\n\t"b": 2\n}\n'


def test_insert_lands_before_a_dangling_comment():
    text = '{\n\t"a": 1\n\t// still to do\n}\n'
    assert apply_set(text, ("b",), 2) == '{\n\t"a": 1,\n\t"b": 2\n\t// still to do\n}\n'


def test_insert_into_an_empty_object():
    assert apply_set("{}\n", ("a",), 1) == '{\n\t"a": 1\n}\n'
    assert apply_set("{}", ("a",), 1) == '{\n\t"a": 1\n}'


def test_insert_into_an_empty_object_holding_only_a_comment():
    text = "{\n\t// nothing yet\n}\n"
    assert apply_set(text, ("a",), 1) == '{\n\t// nothing yet\n\t"a": 1\n}\n'


def test_insert_into_an_empty_nested_object():
    text = '{\n\t"[lua]": {}\n}\n'
    assert apply_set(text, ("[lua]", "a"), 1) == '{\n\t"[lua]": {\n\t\t"a": 1\n\t}\n}\n'


def test_insert_into_a_one_line_object_stays_on_that_line():
    text = '{"a": 1}\n'
    assert apply_set(text, ("b",), 2) == '{"a": 1, "b": 2}\n'


def test_tab_indented_files_keep_tabs():
    text = '{\n\t"a": 1\n}\n'
    out = apply_set(text, ("b",), {"c": 3})
    assert out == '{\n\t"a": 1,\n\t"b": {\n\t\t"c": 3\n\t}\n}\n'


def test_two_space_indented_files_keep_two_spaces():
    text = '{\n  "a": 1\n}\n'
    out = apply_set(text, ("b",), {"c": 3})
    assert out == '{\n  "a": 1,\n  "b": {\n    "c": 3\n  }\n}\n'


def test_four_space_indented_files_keep_four_spaces():
    text = '{\n    "a": 1\n}\n'
    out = apply_set(text, ("b",), {"c": 3})
    assert out == '{\n    "a": 1,\n    "b": {\n        "c": 3\n    }\n}\n'


def test_a_dotted_key_is_one_flat_key():
    text = '{\n\t"editor": {\n\t\t"formatOnSave": false\n\t}\n}\n'
    out = apply_set(text, ("editor.formatOnSave",), True)
    assert parse(out) == {"editor": {"formatOnSave": False}, "editor.formatOnSave": True}
    assert out == (
        '{\n\t"editor": {\n\t\t"formatOnSave": false\n\t},\n\t"editor.formatOnSave": true\n}\n'
    )


def test_nested_path_replaces_inside_a_language_block():
    out = apply_set(COMMENTED, ("[lua]", "editor.tabSize"), 2)
    assert out == COMMENTED.replace('"editor.tabSize": 4', '"editor.tabSize": 2')


def test_nested_path_inserts_at_the_right_depth():
    out = apply_set(COMMENTED, ("[lua]", "editor.insertSpaces"), True)
    assert out == COMMENTED.replace(
        '\t\t"editor.tabSize": 4\n',
        '\t\t"editor.tabSize": 4,\n\t\t"editor.insertSpaces": true\n',
    )
    assert parse(out)["[lua]"] == {"editor.tabSize": 4, "editor.insertSpaces": True}


def test_a_value_full_of_braces_and_escapes_survives_a_neighbours_edit():
    text = '{\n\t"a": "say \\"hi\\" { x } [ y ] // z",\n\t"b": 1\n}\n'
    out = apply_set(text, ("b",), 2)
    assert out == text.replace('"b": 1', '"b": 2')
    assert parse(out) == {"a": 'say "hi" { x } [ y ] // z', "b": 2}


def test_a_value_full_of_braces_and_escapes_can_be_written():
    out = apply_set('{\n\t"a": 1\n}\n', ("b",), 'say "hi" {x} [y]')
    assert parse(out) == {"a": 1, "b": 'say "hi" {x} [y]'}


def test_trailing_commas_elsewhere_are_left_alone():
    text = '{\n\t"a": [1, 2,],\n\t"b": 1\n}\n'
    out = apply_set(text, ("b",), 2)
    assert out == text.replace('"b": 1', '"b": 2')


def test_missing_intermediate_objects_are_created():
    out = apply_set('{\n\t"a": 1\n}\n', ("[toml]", "editor.tabSize"), 2)
    assert out == '{\n\t"a": 1,\n\t"[toml]": {\n\t\t"editor.tabSize": 2\n\t}\n}\n'
    assert parse(out) == {"a": 1, "[toml]": {"editor.tabSize": 2}}


def test_several_missing_intermediate_objects_are_created():
    out = apply_set("{}\n", ("a", "b", "c"), 1)
    assert out == '{\n\t"a": {\n\t\t"b": {\n\t\t\t"c": 1\n\t\t}\n\t}\n}\n'
    assert parse(out) == {"a": {"b": {"c": 1}}}


def test_a_non_object_on_the_path_is_refused():
    with pytest.raises(ValueError, match="'a' is not an object"):
        apply_set('{\n\t"a": 1\n}\n', ("a", "b"), 2)
    with pytest.raises(ValueError, match="'a' is not an object"):
        apply_set('{\n\t"a": [1]\n}\n', ("a", "b", "c"), 2)


def test_a_document_that_is_not_an_object_is_refused():
    with pytest.raises(ValueError, match="root is not a JSON object"):
        apply_set("[]\n", ("a",), 1)


def test_an_empty_path_is_refused(tmp_path):
    with pytest.raises(ValueError, match="at least one key"):
        set_key(str(tmp_path / "x.json"), (), 1)
    with pytest.raises(ValueError, match="at least one key"):
        remove_key(str(tmp_path / "x.json"), ())


def test_remove_the_first_member():
    text = '{\n\t"a": 1,\n\t"b": 2,\n\t"c": 3\n}\n'
    assert apply_remove(text, ("a",)) == '{\n\t"b": 2,\n\t"c": 3\n}\n'


def test_remove_a_middle_member():
    text = '{\n\t"a": 1,\n\t"b": 2,\n\t"c": 3\n}\n'
    assert apply_remove(text, ("b",)) == '{\n\t"a": 1,\n\t"c": 3\n}\n'


def test_remove_the_last_member_drops_the_comma_before_it():
    text = '{\n\t"a": 1,\n\t"b": 2\n}\n'
    assert apply_remove(text, ("b",)) == '{\n\t"a": 1\n}\n'


def test_remove_the_only_member():
    assert apply_remove('{\n\t"a": 1\n}\n', ("a",)) == "{\n}\n"


def test_remove_the_last_member_keeps_the_comment_on_the_line_above():
    text = '{\n\t"a": 1, // note\n\t"b": 2\n}\n'
    assert apply_remove(text, ("b",)) == '{\n\t"a": 1 // note\n}\n'


def test_remove_takes_the_members_own_trailing_comment_with_it():
    text = '{\n\t"a": 1, // gone\n\t"b": 2\n}\n'
    assert apply_remove(text, ("a",)) == '{\n\t"b": 2\n}\n'


def test_remove_keeps_comments_that_belong_to_other_members():
    out = apply_remove(COMMENTED, ("[lua]", "editor.tabSize"))
    assert out == COMMENTED.replace('\t\t"editor.tabSize": 4\n', "")
    assert parse(out)["[lua]"] == {}


def test_remove_from_a_one_line_object():
    assert apply_remove('{"a": 1, "b": 2}\n', ("a",)) == '{ "b": 2}\n'
    assert apply_remove('{"a": 1, "b": 2}\n', ("b",)) == '{"a": 1 }\n'


def test_remove_a_member_whose_value_is_a_structure():
    out = apply_remove(COMMENTED, ("[lua]",))
    assert parse(out) == {
        "git.autofetch": True,
        "files.associations": {"*.zsh": "shellscript"},
    }
    assert "// git" in out
    assert "/* a block */" in out


def test_remove_reports_nothing_to_do_for_a_key_that_is_not_there():
    assert apply_remove('{\n\t"a": 1\n}\n', ("b",)) is None
    assert apply_remove('{\n\t"a": 1\n}\n', ("x", "y")) is None
    assert apply_remove('{\n\t"a": 1\n}\n', ("a", "b")) is None


def test_removal_leaves_a_document_that_still_parses():
    text = '{\n\t"a": 1,\n\t"b": 2,\n\t"c": 3\n}\n'
    for key in ("a", "b", "c"):
        assert parse(apply_remove(text, (key,))) == {
            name: value for name, value in parse(text).items() if name != key
        }


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("git.autofetch",), False),
        (("brand.new",), "hello"),
        (("[lua]", "editor.tabSize"), 2),
        (("[lua]", "editor.defaultFormatter"), "sumneko.lua"),
        (("files.associations", "*.hujson"), "jsonc"),
        (("[toml]", "editor.tabSize"), 2),
    ],
)
def test_set_changes_exactly_one_key_and_nothing_else(path, value):
    document = parse(COMMENTED)
    branch = document
    for key in path[:-1]:
        branch = branch.setdefault(key, {})
    branch[path[-1]] = value
    assert parse(apply_set(COMMENTED, path, value)) == document


def test_set_key_creates_a_missing_file(tmp_path):
    target = tmp_path / "deep" / "settings.json"
    set_key(str(target), ("editor.fontSize",), 14)
    assert read_back(str(target)) == '{\n\t"editor.fontSize": 14\n}\n'


def test_remove_key_on_a_missing_file_is_a_no_op(tmp_path):
    target = tmp_path / "settings.json"
    assert remove_key(str(target), ("a",)) is False
    assert not target.exists()


def test_remove_key_on_a_missing_key_leaves_the_file_alone(tmp_path):
    path_to_file = copy_to(tmp_path, COMMENTED)
    assert remove_key(path_to_file, ("nope",)) is False
    assert read_back(path_to_file) == COMMENTED


def test_set_then_remove_round_trips_to_the_original_bytes(tmp_path):
    path_to_file = copy_to(tmp_path, COMMENTED)
    set_key(path_to_file, ("json.schemas",), [1, 2])
    assert remove_key(path_to_file, ("json.schemas",)) is True
    assert read_back(path_to_file) == COMMENTED


@needs_repo_file
def test_real_settings_replace_rewrites_one_value(tmp_path):
    old = real_text()
    path_to_file = copy_to(tmp_path, old)
    set_key(path_to_file, ("editor.formatOnSave",), False)
    new = read_back(path_to_file)
    assert new.replace('"editor.formatOnSave": false', '"editor.formatOnSave": true', 1) == old
    assert parse(new)["editor.formatOnSave"] is False
    assert parse(new)["[lua]"]["editor.formatOnSave"] is True


@needs_repo_file
def test_real_settings_insert_adds_one_line_and_one_comma(tmp_path):
    old = real_text()
    path_to_file = copy_to(tmp_path, old)
    set_key(path_to_file, ("editor.fontSize",), 14)
    new = read_back(path_to_file)
    assert parse(new) == dict(parse(old), **{"editor.fontSize": 14})
    dropped, gained = changed(old, new)
    assert len(dropped) == 1 and len(gained) == 2, "more than one line moved"
    assert gained[0] == dropped[0] + ",", "the key that was last gained only a comma"
    assert gained[1] == '\t"editor.fontSize": 14'


@needs_repo_file
def test_real_settings_nested_insert_keeps_every_other_byte(tmp_path):
    old = real_text()
    path_to_file = copy_to(tmp_path, old)
    set_key(path_to_file, ("[lua]", "editor.tabSize"), 2)
    new = read_back(path_to_file)
    assert parse(new)["[lua]"]["editor.tabSize"] == 2
    dropped, gained = changed(old, new)
    assert len(dropped) == 1 and len(gained) == 2, "more than one line moved"
    assert gained[0] == dropped[0] + ",", "the member that was last gained only a comma"
    assert gained[1] == '\t\t"editor.tabSize": 2'


@needs_repo_file
def test_real_settings_keeps_its_tab_indent_for_a_new_structure(tmp_path):
    # A key the real file cannot already carry, so this keeps testing the indent
    # of a structure being written rather than which language blocks
    # settings.json happens to have today.
    path_to_file = copy_to(tmp_path, real_text())
    set_key(path_to_file, ("[dotfile-probe]",), {"editor.defaultFormatter": "vendor.probe"})
    new = read_back(path_to_file)
    assert '\t"[dotfile-probe]": {\n\t\t"editor.defaultFormatter": "vendor.probe"\n\t}\n' in new
    assert parse(new)["[dotfile-probe]"] == {"editor.defaultFormatter": "vendor.probe"}


@needs_repo_file
def test_real_settings_round_trip_through_set_and_remove(tmp_path):
    old = real_text()
    path_to_file = copy_to(tmp_path, old)
    set_key(path_to_file, ("editor.fontSize",), 14)
    assert remove_key(path_to_file, ("editor.fontSize",)) is True
    assert read_back(path_to_file) == old


def test_crlf_files_keep_their_line_endings():
    text = '{\r\n\t"a": 1\r\n}\r\n'
    assert apply_set(text, ("a",), 9) == '{\r\n\t"a": 9\r\n}\r\n'
    grown = apply_set(text, ("b",), 2)
    assert grown == '{\r\n\t"a": 1,\r\n\t"b": 2\r\n}\r\n'
    assert apply_remove(grown, ("b",)) == text


def test_crlf_files_keep_their_line_endings_for_a_nested_value():
    text = '{\r\n\t"a": 1\r\n}\r\n'
    out = apply_set(text, ("b",), {"c": 3})
    assert out == '{\r\n\t"a": 1,\r\n\t"b": {\r\n\t\t"c": 3\r\n\t}\r\n}\r\n'
    assert parse(out) == {"a": 1, "b": {"c": 3}}


def test_a_unicode_key_and_value_round_trip():
    text = '{\n\t"naïve": 1\n}\n'
    out = apply_set(text, ("café",), "über")
    assert out == '{\n\t"naïve": 1,\n\t"café": "über"\n}\n'
    assert parse(out) == {"naïve": 1, "café": "über"}


def test_a_duplicate_key_edits_the_one_that_wins():
    text = '{\n\t"a": 1,\n\t"a": 2\n}\n'
    assert apply_set(text, ("a",), 3) == '{\n\t"a": 1,\n\t"a": 3\n}\n'
    assert apply_remove(text, ("a",)) == '{\n\t"a": 1\n}\n'
