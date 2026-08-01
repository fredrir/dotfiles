import pytest

from tools.readme import fastfetch

README = """# repo

<!-- fastfetch:start -->

```
stale
```

<!-- fastfetch:end -->

tail
"""


@pytest.fixture
def readme(monkeypatch, tmp_path):
    target = tmp_path / "README.md"
    target.write_text(README)
    monkeypatch.setattr(fastfetch, "readme_path", lambda: str(target))
    monkeypatch.setattr(fastfetch, "render", lambda: "fresh block")
    monkeypatch.setattr(fastfetch.shutil, "which", lambda name: "/usr/bin/fastfetch")
    return target


def test_replaces_the_marker_block(readme, capsys):
    fastfetch.update()
    text = readme.read_text()
    assert "fresh block" in text
    assert "stale" not in text
    assert text.startswith("# repo\n")
    assert text.endswith("tail\n")
    assert "Updated fastfetch preview" in capsys.readouterr().out


def test_second_run_is_idempotent(readme, capsys):
    fastfetch.update()
    before = readme.read_text()
    fastfetch.update()
    assert readme.read_text() == before
    assert "already up to date" in capsys.readouterr().out


def test_missing_markers_fail(monkeypatch, tmp_path):
    target = tmp_path / "README.md"
    target.write_text("no markers here\n")
    monkeypatch.setattr(fastfetch, "readme_path", lambda: str(target))
    monkeypatch.setattr(fastfetch, "render", lambda: "block")
    monkeypatch.setattr(fastfetch.shutil, "which", lambda name: "/usr/bin/fastfetch")
    with pytest.raises(SystemExit):
        fastfetch.update()


def test_skips_without_fastfetch(monkeypatch, capsys):
    monkeypatch.setattr(fastfetch.shutil, "which", lambda name: None)
    fastfetch.update()
    assert "skipping" in capsys.readouterr().out


def test_visible_width_counts_terminal_cells():
    assert fastfetch.visible_width("abc") == 3
    assert fastfetch.visible_width("") == 2
    assert fastfetch.visible_width("テ") == 2
