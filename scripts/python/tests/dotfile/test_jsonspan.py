import pytest

from tools.dotfile.jsonspan import (
    container_span,
    detect_indent,
    key_span,
    members,
    value_span,
)

DOC = (
    "{\n"
    "\t// a note\n"
    '\t"editor.formatOnSave": true,\n'
    '\t"[lua]": {\n'
    '\t\t"editor.tabSize": 4 /* four */\n'
    "\t},\n"
    '\t"files.associations": {"*.zsh": "shellscript"},\n'
    "}\n"
)


def slice_at(text, span):
    return text[span[0] : span[1]]


def test_key_span_covers_the_whole_member():
    assert slice_at(DOC, key_span(DOC, ("editor.formatOnSave",))) == '"editor.formatOnSave": true'


def test_value_span_covers_only_the_value():
    assert slice_at(DOC, value_span(DOC, ("editor.formatOnSave",))) == "true"


def test_a_dot_is_part_of_the_key_not_a_separator():
    assert key_span(DOC, ("editor", "formatOnSave")) is None
    text = '{\n\t"editor": {"formatOnSave": false},\n\t"editor.formatOnSave": true\n}\n'
    assert slice_at(text, value_span(text, ("editor.formatOnSave",))) == "true"
    assert slice_at(text, value_span(text, ("editor", "formatOnSave"))) == "false"


def test_nested_path_stops_before_a_trailing_block_comment():
    assert slice_at(DOC, key_span(DOC, ("[lua]", "editor.tabSize"))) == '"editor.tabSize": 4'


def test_missing_key_and_missing_ancestor():
    assert key_span(DOC, ("nope",)) is None
    assert key_span(DOC, ("nope", "deeper")) is None
    assert container_span(DOC, ("nope", "deeper")) is None


def test_an_ancestor_that_is_not_an_object():
    assert key_span(DOC, ("editor.formatOnSave", "x")) is None
    assert container_span(DOC, ("editor.formatOnSave", "x")) is None


def test_keys_that_only_appear_inside_comments_are_ignored():
    text = '{\n\t// "a": 1\n\t/* "a": 2 */\n\t"a": 3\n}\n'
    assert slice_at(text, key_span(text, ("a",))) == '"a": 3'


def test_a_comment_may_sit_between_the_key_and_the_value():
    text = '{\n\t"a" /* here */ : /* and here */ 7\n}\n'
    assert slice_at(text, value_span(text, ("a",))) == "7"


def test_escaped_quotes_in_a_key_name():
    text = '{\n\t"say \\"hi\\"": 1,\n\t"b": 2\n}\n'
    assert slice_at(text, key_span(text, ('say "hi"',))) == '"say \\"hi\\"": 1'


def test_markers_inside_a_string_value_are_not_syntax():
    text = '{\n\t"a": "{ } [ ] // /* \\" ,",\n\t"b": 2\n}\n'
    assert slice_at(text, value_span(text, ("a",))) == '"{ } [ ] // /* \\" ,"'
    assert slice_at(text, value_span(text, ("b",))) == "2"


def test_a_value_that_is_a_nested_structure():
    text = '{\n\t"a": {"b": [1, {"c": 2}], "d": "}"},\n\t"e": 3\n}\n'
    assert slice_at(text, value_span(text, ("a",))) == '{"b": [1, {"c": 2}], "d": "}"}'
    assert slice_at(text, value_span(text, ("e",))) == "3"


def test_a_brace_inside_a_comment_does_not_close_the_object():
    text = '{\n\t"a": {\n\t\t/* } not the end */\n\t\t"b": 1\n\t},\n\t"c": 2\n}\n'
    assert slice_at(text, value_span(text, ("a", "b"))) == "1"
    assert slice_at(text, value_span(text, ("c",))) == "2"


def test_duplicate_keys_resolve_to_the_last_one():
    text = '{\n\t"a": 1,\n\t"a": 2\n}\n'
    assert slice_at(text, value_span(text, ("a",))) == "2"


def test_duplicate_objects_descend_into_the_last_one():
    text = '{\n\t"o": {"a": 1},\n\t"o": {"a": 2}\n}\n'
    assert slice_at(text, value_span(text, ("o", "a"))) == "2"


def test_trailing_commas_are_tolerated():
    text = '{\n\t"a": [1, 2,],\n\t"b": {"c": 3,},\n}\n'
    assert slice_at(text, value_span(text, ("a",))) == "[1, 2,]"
    assert slice_at(text, value_span(text, ("b", "c"))) == "3"


def test_scalar_values_of_every_shape():
    text = '{"a": -1.5e+10, "b": null, "c": false, "d": 0}'
    assert slice_at(text, value_span(text, ("a",))) == "-1.5e+10"
    assert slice_at(text, value_span(text, ("b",))) == "null"
    assert slice_at(text, value_span(text, ("c",))) == "false"
    assert slice_at(text, value_span(text, ("d",))) == "0"


def test_container_span_is_the_body_of_the_parent_object():
    span = container_span(DOC, ("[lua]", "editor.tabSize"))
    assert slice_at(DOC, span) == '\n\t\t"editor.tabSize": 4 /* four */\n\t'


def test_container_span_of_a_top_level_key_is_the_root_body():
    start, end = container_span(DOC, ("anything",))
    assert DOC[start - 1] == "{"
    assert DOC[end] == "}"


def test_container_span_of_a_key_that_does_not_exist_yet():
    assert container_span(DOC, ("[lua]", "brand.new")) == container_span(
        DOC, ("[lua]", "editor.tabSize")
    )


def test_an_empty_path_has_no_span():
    assert key_span(DOC, ()) is None
    assert value_span(DOC, ()) is None
    assert container_span(DOC, ()) is None


def test_members_lists_every_key_in_order():
    body = container_span(DOC, ("x",))
    assert [item[0] for item in members(DOC, body)] == [
        "editor.formatOnSave",
        "[lua]",
        "files.associations",
    ]


def test_members_of_an_empty_body():
    assert members("{}", (1, 1)) == []


def test_members_report_usable_offsets():
    text = '{\n\t"a": 12\n}\n'
    key, key_start, value_start, value_end = members(text, container_span(text, ("a",)))[0]
    assert key == "a"
    assert text[key_start] == '"'
    assert text[value_start:value_end] == "12"


def test_detect_indent_reads_tabs():
    assert detect_indent('{\n\t"a": 1\n}\n') == "\t"


def test_detect_indent_reads_spaces():
    assert detect_indent('{\n  "a": 1\n}\n') == "  "
    assert detect_indent('{\n    "a": 1\n}\n') == "    "


def test_detect_indent_skips_blank_lines():
    assert detect_indent('{\n   \n\t"a": 1\n}\n') == "\t"


def test_detect_indent_defaults_to_a_tab():
    assert detect_indent('{"a": 1}\n') == "\t"
    assert detect_indent("{}") == "\t"


def test_a_document_that_is_not_an_object_has_no_spans():
    assert key_span("[1, 2]", ("a",)) is None
    assert key_span("", ("a",)) is None
    assert container_span("// only a comment\n", ("a",)) is None


def test_unterminated_input_is_rejected():
    with pytest.raises(ValueError, match="unterminated string"):
        key_span('{"a": "oops', ("a",))
    with pytest.raises(ValueError, match="unterminated block comment"):
        key_span('{"a": 1 /* oops', ("a",))
    with pytest.raises(ValueError, match="unterminated container"):
        key_span('{"a": 1', ("a",))


def test_malformed_members_are_rejected():
    with pytest.raises(ValueError, match="expected a key"):
        key_span("{1: 2}", ("a",))
    with pytest.raises(ValueError, match="expected ':'"):
        key_span('{"a" 2}', ("a",))
    with pytest.raises(ValueError, match="expected a value"):
        key_span('{"a": , "b": 1}', ("a",))
