import re

import pytest

from tools.dotfile import render
from tools.dotfile.render import change, key, value

# The JSON string grammar, spelled out independently of render's own scanner, so the
# escape-boundary tests check the output rather than restate the implementation.
ATOM = re.compile(r"\\u[0-9a-fA-F]{4}|\\.|.", re.DOTALL)

TRICKY = 'a"b\\c\x01d\te'


def boundaries(text):
    """Every offset in `text` that does not fall inside a backslash escape."""
    offsets, index = [0], 0
    for atom in ATOM.findall(text):
        index += len(atom)
        offsets.append(index)
    return offsets


def test_strings_keep_their_quotes():
    assert value("hello") == '"hello"'


def test_scalars_render_as_json():
    assert [value(item) for item in (14, 3.5, True, False, None)] == [
        "14",
        "3.5",
        "true",
        "false",
        "null",
    ]


def test_containers_collapse_compactly():
    assert value(["a", "b", "c"]) == '["a", "b", "c"]'
    assert value({"x": 1, "y": 2}) == '{"x": 1, "y": 2}'
    assert value({"outer": {"inner": [1, 2]}}) == '{"outer": {"inner": [1, 2]}}'


def test_empty_containers_render_as_themselves():
    assert value([]) == "[]"
    assert value({}) == "{}"
    assert value("") == '""'


def test_unicode_survives_as_itself():
    assert value("café") == '"café"'
    assert value(["日本語", "→", "😀"]) == '["日本語", "→", "😀"]'


def test_quotes_and_control_characters_are_escaped():
    assert value('say "hi"') == '"say \\"hi\\""'
    assert value("a\tb\nc") == '"a\\tb\\nc"'
    assert value("a\x01b") == '"a\\u0001b"'
    assert value("back\\slash") == '"back\\\\slash"'


def test_a_value_that_is_not_json_still_renders():
    assert value({1}) == '"{1}"'


def test_no_width_means_no_clipping():
    long = "z" * 500
    assert value(long) == f'"{long}"'
    assert change("modify", long, long) == f'"{long}" → "{long}"'


def test_a_value_that_fits_is_left_alone():
    assert value("abc", 5) == '"abc"'
    assert value("abc", 500) == '"abc"'


def test_clipping_marks_the_cut_with_an_ellipsis():
    assert value("abcdefgh", 5) == '"abc…'
    assert len(value("abcdefgh", 5)) == 5


def test_clipping_keeps_an_escaped_quote_whole():
    assert value('ab"cd') == '"ab\\"cd"'
    assert value('ab"cd', 6) == '"ab\\"…'
    assert value('ab"cd', 5) == '"ab…'


def test_clipping_keeps_a_unicode_escape_whole():
    assert value("a\x01b", 9) == '"a\\u0001…'
    assert value("a\x01b", 8) == '"a…'


@pytest.mark.parametrize("width", range(1, 26))
def test_clipping_always_lands_on_an_escape_boundary(width):
    full = render.dumps(TRICKY)
    text = value(TRICKY, width)
    if text == full:
        return
    assert text.endswith("…")
    assert len(text) - 1 in boundaries(full)


@pytest.mark.parametrize("width", [-3, 0, 1, 2, 3])
def test_very_small_widths_degrade_instead_of_crashing(width):
    for kind in ("add", "modify", "delete", "conflict"):
        assert len(change(kind, "abc", "def", width)) <= max(width, 0)


@pytest.mark.parametrize("kind", ["add", "modify", "delete", "conflict"])
@pytest.mark.parametrize("width", [0, 1, 2, 4, 7, 12, 20, 33, 54])
def test_nothing_ever_renders_wider_than_its_budget(kind, width):
    detail = change(kind, "o" * 60, {"deep": ["value", 1, None]}, width)
    assert len(detail) <= width


def test_a_flat_key_is_quoted():
    assert key(("git.autofetch",)) == '"git.autofetch"'


def test_a_nested_key_joins_with_a_slash():
    assert key(("[lua]", "editor.tabSize")) == '"[lua]/editor.tabSize"'
    assert key(("a", "b", "c")) == '"a/b/c"'


def test_dots_inside_a_key_are_left_alone():
    assert key(("editor.formatOnSave",)) == '"editor.formatOnSave"'
    assert key(("[python]", "editor.formatOnSave")) == '"[python]/editor.formatOnSave"'


def test_key_segments_need_not_be_strings():
    assert key(("recent", 0, "path")) == '"recent/0/path"'


def test_a_key_containing_a_quote_is_escaped():
    assert key(('say "hi"',)) == '"say \\"hi\\""'


def test_add_shows_the_new_value():
    assert change("add", None, 14) == "14"
    assert change("add", None, ["fredrir", "dotfile"]) == '["fredrir", "dotfile"]'


def test_modify_shows_both_sides():
    assert change("modify", True, False) == "true → false"
    assert change("modify", "old", "new") == '"old" → "new"'


def test_delete_marks_the_removal():
    assert change("delete", "gone", None) == '"gone" → (removed)'


def test_conflict_labels_both_sides():
    assert change("conflict", "a@b.c", "d@e.f") == 'repo: "a@b.c"  live: "d@e.f"'


def test_an_unknown_kind_reads_as_a_modify():
    assert change("surprise", 1, 2) == "1 → 2"


def test_a_tiny_side_is_never_starved_by_a_huge_one():
    detail = change("modify", 1, "z" * 200, 40)
    assert detail.startswith("1 → ")
    assert len(detail) == 40

    detail = change("modify", "z" * 200, 1, 40)
    assert detail.endswith(" → 1")
    assert len(detail) == 40


def test_two_huge_sides_share_the_width_evenly():
    detail = change("modify", "y" * 200, "z" * 200, 41)
    left, right = detail.split(" → ")
    assert len(detail) == 41
    assert abs(len(left) - len(right)) <= 1


def test_delete_never_clips_the_removed_marker():
    detail = change("delete", "z" * 200, None, 30)
    assert detail.endswith(" → (removed)")
    assert len(detail) == 30


def test_conflict_never_clips_its_labels():
    detail = change("conflict", "y" * 200, "z" * 200, 44)
    assert detail.startswith("repo: ")
    assert "  live: " in detail
    assert len(detail) == 44


def test_conflict_gives_a_short_side_its_whole_length():
    detail = change("conflict", "a@b.c", "d" * 100, 46)
    assert detail.startswith('repo: "a@b.c"  live: ')
    assert len(detail) == 46


def test_split_hands_the_surplus_to_the_longer_side():
    assert render.split(20, "abc", "de") == (3, 2)
    assert render.split(10, "abc", "z" * 40) == (3, 7)
    assert render.split(10, "z" * 40, "abc") == (7, 3)
    assert render.split(10, "y" * 40, "z" * 40) == (5, 5)


def test_it_composes_the_status_detail_lines():
    added = f"+ {key(('editor.fontSize',))}: {change('add', None, 14)}"
    changed = f"~ {key(('git.autofetch',))}: {change('modify', True, False)}"
    clash = f"! {key(('user.email',))}  {change('conflict', 'a@b.c', 'd@e.f')}"
    assert added == '+ "editor.fontSize": 14'
    assert changed == '~ "git.autofetch": true → false'
    assert clash == '! "user.email"  repo: "a@b.c"  live: "d@e.f"'
