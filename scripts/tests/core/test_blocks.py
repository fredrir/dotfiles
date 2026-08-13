import pytest

from tools.core import blocks


def test_entries_carry_their_block_and_line_number():
    entries = blocks.scan(["host {", "  a = 1", "", "  b = 2", "}"])

    assert [(entry.block, entry.number, entry.text, entry.opens) for entry in entries] == [
        ("host", 1, "", True),
        ("host", 2, "a = 1", False),
        ("host", 4, "b = 2", False),
    ]


def test_comments_are_stripped_by_default():
    entries = blocks.scan(["# leading", "host {", "  a = 1 # trailing", "}"])

    assert [entry.text for entry in entries if not entry.opens] == ["a = 1"]


def test_comments_are_kept_when_disabled():
    entries = blocks.scan(["host {", "  a = 1 # kept", "}"], comments=False)

    assert [entry.text for entry in entries if not entry.opens] == ["a = 1 # kept"]


def test_an_alternate_open_suffix_leaves_other_lines_alone():
    entries = blocks.scan(["group {", "  brace{", "}"], comments=False, open_suffix=" {")

    assert [(entry.block, entry.text) for entry in entries] == [
        ("group", ""),
        ("group", "brace{"),
    ]


def test_split_returns_the_whole_line_when_the_separator_is_absent():
    entry = blocks.Entry("group", 2, "plain")

    assert entry.split("=") == ("plain", "")


def test_split_trims_both_sides():
    entry = blocks.Entry("group", 2, "key   =   value")

    assert entry.split("=") == ("key", "value")


def test_fields_splits_on_whitespace():
    entry = blocks.Entry("allow", 2, "path/to/file  label")

    assert entry.fields() == ["path/to/file", "label"]


@pytest.mark.parametrize(
    ("lines", "kind", "number"),
    [
        (["}"], blocks.UNEXPECTED_CLOSE, 1),
        (["a {", "b {"], blocks.NESTED, 2),
        (["entry"], blocks.OUTSIDE, 1),
        (["a {", "  entry"], blocks.UNTERMINATED, 2),
    ],
)
def test_structural_errors_report_their_kind_and_line(lines, kind, number):
    with pytest.raises(blocks.BlockError) as error:
        blocks.scan(lines)

    assert error.value.kind == kind
    assert error.value.number == number


def test_an_unterminated_block_reports_its_name():
    with pytest.raises(blocks.BlockError) as error:
        blocks.scan(["archie {", "  a = 1"])

    assert error.value.block == "archie"


def test_reading_a_missing_file_returns_nothing(tmp_path):
    assert blocks.read(str(tmp_path / "absent.dotfile")) == []
