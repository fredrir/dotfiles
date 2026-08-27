import os

import pytest

from tools.core.paths import dotfiles_root
from tools.surface import docs, entry, pages, rust


def test_a_block_is_replaced_without_touching_the_prose_around_it():
    text = (
        "# tool\n\nSomething written by hand.\n\n"
        "<!-- cli:commands:start -->\nold\n<!-- cli:commands:end -->\n\nA closing word.\n"
    )
    updated = docs.replace_block(text, docs.COMMANDS_BLOCK, "new")
    assert "Something written by hand." in updated
    assert "A closing word." in updated
    assert "old" not in updated
    assert "new" in updated


def test_a_file_without_markers_is_left_for_the_generator_to_rewrite():
    assert docs.replace_block("# tool\n\nno markers here\n", docs.COMMANDS_BLOCK, "new") is None


def test_a_pipe_in_a_description_does_not_end_its_cell():
    rendered = docs.table(["Flag", "Description"], [["`--override`", "one of `a|b|none`"]])
    assert r"one of `a\|b\|none`" in rendered


def test_a_table_lines_its_columns_up():
    rendered = docs.table(["Command", "Description"], [["`a`", "Short."], ["`longer`", "Longer."]])
    widths = {len(line) for line in rendered.splitlines()}
    assert len(widths) == 1


def test_generating_twice_changes_nothing_the_second_time(tmp_path, monkeypatch):
    monkeypatch.setenv("DOTFILE_ROOT", str(tmp_path))
    trees = entry.trees()
    first, _missing = docs.write(trees)
    assert first
    second, _missing = docs.write(trees)
    assert second == []


def test_a_stale_page_is_reported_without_being_written(tmp_path, monkeypatch):
    monkeypatch.setenv("DOTFILE_ROOT", str(tmp_path))
    trees = entry.trees()
    docs.write(trees)
    page = tmp_path / docs.DOCS_DIR / "dotfile.md"
    page.write_text(page.read_text().replace("Manages this repository", "Manages"))
    changed, _missing = docs.write(trees, check=True)
    assert "docs/cli/dotfile.md" in changed
    assert "Manages this repository" not in page.read_text()


def test_a_tool_that_is_not_built_leaves_its_page_alone(tmp_path, monkeypatch):
    monkeypatch.setenv("DOTFILE_ROOT", str(tmp_path))
    monkeypatch.setattr(rust, "binary", lambda program: "")
    changed, missing = docs.write(entry.trees(), check=True)
    assert "count" in missing
    assert "docs/cli/count.md" not in changed


@pytest.mark.parametrize("page", pages.PAGES, ids=lambda page: page.name)
def test_the_repository_pages_are_up_to_date(page):
    roots = [(program, entry.trees().get(program) or rust.tree(program)) for program in page.programs]
    if any(tree is None for _program, tree in roots):
        pytest.skip(f"{page.name} needs a tool this checkout has not built")
    path = os.path.join(str(dotfiles_root()), docs.DOCS_DIR, f"{page.name}.md")
    assert docs.page_text(page, roots, docs.read(path)) == docs.read(path)
