from pathlib import Path

import pytest

from tools.core.menu import Pick
from tools.transcript import cli


@pytest.fixture
def candidates(monkeypatch):
    found = [(f"proj{number}", Path(f"/tmp/p{number}")) for number in range(23)]
    monkeypatch.setattr(cli, "_untracked_candidates", lambda: list(found))
    return cli._Candidates()


def menu_pick(option):
    return (Pick("menu", cli._MENU_NAMES.index(option), option),)


def test_the_root_column_is_the_menu(candidates):
    column = cli._expand(candidates)(())
    assert column.kind == "menu"
    assert column.options == cli._MENU_NAMES


@pytest.mark.parametrize("option", ["capture", "import", "list", "migrate", "sync"])
def test_most_entries_are_leaves(candidates, option):
    assert cli._expand(candidates)(menu_pick(option)) is None


def test_add_opens_the_candidate_column(candidates):
    column = cli._expand(candidates)(menu_pick("add"))
    assert column.kind == "candidate"
    assert column.options[:2] == ["proj0", "proj1"]
    assert column.options[-2:] == ["show 10 more…", cli.ENTER_PATH]


def test_revealing_a_page_moves_the_cursor_to_the_first_new_row(candidates):
    column = cli._expand(candidates)(menu_pick("add"))
    more = column.options.index("show 10 more…")
    assert candidates.reveal(more) is True
    assert candidates.start == (cli._MENU_NAMES.index("add"), 10)
    grown = cli._expand(candidates)(menu_pick("add"))
    assert grown.options[grown.index] == "proj10"
    assert grown.options[-2:] == ["show 3 more…", cli.ENTER_PATH]


def test_the_last_page_drops_the_reveal_row(candidates):
    candidates.reveal(10)
    candidates.reveal(20)
    column = cli._expand(candidates)(menu_pick("add"))
    assert column.options[-1] == cli.ENTER_PATH
    assert not any(option.startswith("show ") for option in column.options)


def test_choosing_a_project_is_not_a_reveal(candidates):
    cli._expand(candidates)(menu_pick("add"))
    assert candidates.reveal(3) is False


def test_a_candidate_pick_resolves_to_its_directory(candidates):
    cli._expand(candidates)(menu_pick("add"))
    assert candidates.directory(Pick("candidate", 2, "proj2")) == Path("/tmp/p2")


def test_rm_offers_the_tracked_projects(candidates, monkeypatch):
    monkeypatch.setattr(cli.config, "project_list", lambda: ["alpha", "beta"])
    column = cli._expand(candidates)(menu_pick("rm"))
    assert (column.kind, column.options) == ("project", ["alpha", "beta"])


def test_nothing_tracked_becomes_a_note(candidates, monkeypatch):
    monkeypatch.setattr(cli.config, "project_list", list)
    column = cli._expand(candidates)(menu_pick("rm"))
    assert (column.kind, column.options) == ("note", ["no tracked projects"])
