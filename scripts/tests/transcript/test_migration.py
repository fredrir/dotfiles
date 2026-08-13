import pytest

from tools.transcript import migration


def configure(tmp_path, monkeypatch):
    vault = tmp_path / "vault"
    config_file = tmp_path / "migration-config.toml"
    config_file.write_text(
        'projects = ["dotfiles"]\n'
        "\n"
        "[groups]\n"
        'dotfiles = ["dotfiles"]\n'
        "\n"
        "[destinations]\n"
        'dotfiles = "Dotfiles/Agents"\n'
    )
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(vault))
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    return vault


def test_migration_moves_group_contents_and_removes_empty_source(tmp_path, monkeypatch):
    vault = configure(tmp_path, monkeypatch)
    source = vault / "Transcripts/dotfiles/2026-07/codex/17-note.md"
    attachment = vault / "Transcripts/dotfiles/assets/context.txt"
    source.parent.mkdir(parents=True)
    attachment.parent.mkdir(parents=True)
    source.write_text("note")
    attachment.write_text("context")

    moves = migration.plan()
    assert {str(move.destination.relative_to(vault)) for move in moves} == {
        "Dotfiles/Agents/2026-07/codex/17-note.md",
        "Dotfiles/Agents/assets/context.txt",
    }
    assert all(str(move.destination).startswith(str(vault / "Dotfiles/Agents")) for move in moves)
    assert migration.apply(moves) == 2
    assert (vault / "Dotfiles/Agents/2026-07/codex/17-note.md").read_text() == "note"
    assert (vault / "Dotfiles/Agents/assets/context.txt").read_text() == "context"
    assert not (vault / "Transcripts/dotfiles").exists()


def test_migration_refuses_all_moves_when_a_destination_exists(tmp_path, monkeypatch):
    vault = configure(tmp_path, monkeypatch)
    first = vault / "Transcripts/dotfiles/2026-07/codex/17-first.md"
    second = vault / "Transcripts/dotfiles/2026-07/codex/18-second.md"
    conflict = vault / "Dotfiles/Agents/2026-07/codex/18-second.md"
    first.parent.mkdir(parents=True)
    conflict.parent.mkdir(parents=True)
    first.write_text("first")
    second.write_text("second")
    conflict.write_text("existing")

    moves = migration.plan()
    assert [move.destination for move in migration.conflicts(moves)] == [conflict]
    with pytest.raises(FileExistsError):
        migration.apply(moves)
    assert first.exists()
    assert second.exists()
    assert conflict.read_text() == "existing"
