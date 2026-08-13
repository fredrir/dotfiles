from datetime import UTC, datetime

from tools.transcript import redact, vault
from tools.transcript.model import Round, Session, Turn


def make_session(tmp_path):
    source = tmp_path / "session.jsonl"
    source.write_text("{}")
    session = Session(
        provider="claude",
        session_id="abc-123",
        source_path=str(source),
        cwd="/home/fredrir/dotfiles",
        model="claude-fable-5",
        title="Fix the sync script",
        started=datetime(2026, 8, 9, 16, 32, tzinfo=UTC),
    )
    session.rounds = [
        Round(
            timestamp=session.started,
            label="fix it",
            turns=[Turn("me", "You", "fix it"), Turn("turn", "Response", "done")],
        ),
    ]
    return session


def test_slugify():
    assert vault.slugify("Fix the Sync Script!") == "fix-the-sync-script"
    assert vault.slugify("") == "session"
    assert vault.slugify("blåbærsyltetøy på skiva") == "blåbærsyltetøy-på-skiva"


def test_project_of():
    assert vault.project_of("/home/fredrir/dotfiles") == "dotfiles"
    assert vault.project_of("/home/fredrir/dotfiles/shared/obsidian/snippets") == "dotfiles"
    assert vault.project_of("/home/fredrir/projects/ArchTeX/docs") == "ArchTeX"
    assert vault.project_of("") == "Unsorted"


def test_project_of_home_directory():
    from pathlib import Path

    assert vault.project_of(str(Path.home())) == "Home"


def test_save_creates_note_with_frontmatter(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    session = make_session(tmp_path)
    path, existed = vault.save_session(session, "import", redact.redact)
    assert not existed
    assert path.name == "09-fix-the-sync-script.md"
    assert path.parent.name == "claude"
    assert path.parent.parent.name == "2026-08"
    assert path.parent.parent.parent.name == "dotfiles"
    text = path.read_text()
    assert "session: abc-123" in text
    assert "status: inbox" in text
    assert "obsidianUIMode: preview" in text
    assert "cssclasses: transcript" in text
    assert "### 16:32 — fix it" in text


def test_filename_conflicts_get_suffix(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    first = make_session(tmp_path)
    path1, _ = vault.save_session(first, "import", redact.redact)
    second = make_session(tmp_path)
    second.session_id = "def-456"
    path2, _ = vault.save_session(second, "import", redact.redact)
    assert path1.name == "09-fix-the-sync-script.md"
    assert path2.name == "09-fix-the-sync-script (1).md"


def test_aliases_map_paths_to_project_names(tmp_path, monkeypatch):
    config_file = tmp_path / "alias-config.toml"
    config_file.write_text(
        'projects = ["llunde-backend"]\n'
        "\n"
        "[aliases]\n"
        '"llunde-new/backend" = "llunde-backend"\n'
        "\n"
        "[groups]\n"
        'llunde = ["llunde-backend"]\n'
    )
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    assert vault.project_of("/Users/fredrir/llunde-new/backend") == "llunde-backend"
    assert vault.project_of("/Users/fredrir/llunde-new/backend/src/main") == "llunde-backend"
    assert vault.project_of("/Users/fredrir/llunde-new") == "llunde-new"
    assert vault.folder_for("llunde-backend") == "llunde"


def test_nested_groups_add_subfolder(tmp_path, monkeypatch):
    config_file = tmp_path / "nested-config.toml"
    config_file.write_text(
        'nested_groups = ["llunde"]\n'
        "\n"
        "[groups]\n"
        'llunde = ["llunde-backend", "special"]\n'
        'other = ["sndbx"]\n'
    )
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    assert vault.folder_for("llunde-backend") == "llunde/backend"
    assert vault.folder_for("special") == "llunde/special"
    assert vault.folder_for("sndbx") == "other"
    assert vault.folder_for("dotfiles") == "dotfiles"


def test_group_named_member_files_at_group_root(tmp_path, monkeypatch):
    config_file = tmp_path / "root-config.toml"
    config_file.write_text(
        'nested_groups = ["llunde"]\n'
        "\n"
        "[groups]\n"
        'llunde = ["llunde", "llunde-backend"]\n'
    )
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    assert vault.folder_for("llunde") == "llunde"
    assert vault.folder_for("llunde-backend") == "llunde/backend"


def test_nested_group_note_path(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    config_file = tmp_path / "nested-path-config.toml"
    config_file.write_text(
        'projects = ["llunde-backend"]\n'
        'nested_groups = ["llunde"]\n'
        "\n"
        "[aliases]\n"
        '"llunde-new/backend" = "llunde-backend"\n'
        "\n"
        "[groups]\n"
        'llunde = ["llunde-backend"]\n'
    )
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    session = make_session(tmp_path)
    session.cwd = "/Users/fredrir/llunde-new/backend"
    path, _ = vault.save_session(session, "sync", redact.redact)
    relative = path.relative_to(tmp_path / "vault" / "Transcripts")
    assert str(relative) == "llunde/backend/2026-08/claude/09-fix-the-sync-script.md"
    assert "project: llunde-backend" in path.read_text()


def test_groups_nest_projects(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    config_file = tmp_path / "config.toml"
    config_file.write_text('[groups]\nRice = ["dotfiles", "theme"]\n')
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    session = make_session(tmp_path)
    path, _ = vault.save_session(session, "import", redact.redact)
    assert path.parent.parent.parent.name == "Rice"
    assert "project: dotfiles" in path.read_text()


def test_group_destination_replaces_transcripts_group_root(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    config_file = tmp_path / "destination-config.toml"
    config_file.write_text(
        '[groups]\ndotfiles = ["dotfiles"]\n\n[destinations]\ndotfiles = "Dotfiles/Agents"\n'
    )
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    session = make_session(tmp_path)
    path, existed = vault.save_session(session, "sync", redact.redact)
    assert not existed
    relative = path.relative_to(tmp_path / "vault")
    assert str(relative) == "Dotfiles/Agents/2026-08/claude/09-fix-the-sync-script.md"

    path_again, existed = vault.save_session(session, "sync", redact.redact)
    assert existed
    assert path_again == path


def test_group_destination_must_stay_inside_vault(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    config_file = tmp_path / "unsafe-destination-config.toml"
    config_file.write_text('[destinations]\ndotfiles = "../outside"\n')
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(config_file))
    try:
        vault.directory_for("dotfiles")
    except ValueError as error:
        assert "must stay inside the vault" in str(error)
    else:
        raise AssertionError("unsafe destination was accepted")


def test_resave_updates_in_place_and_preserves_edits(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    session = make_session(tmp_path)
    path, _ = vault.save_session(session, "sync", redact.redact)
    edited = path.read_text().replace("status: inbox", "status: kept\nrating: 5")
    path.write_text(edited)
    session.rounds[0].turns[1].body = "done differently"
    path2, existed = vault.save_session(session, "sync", redact.redact)
    assert existed
    assert path2 == path
    text = path.read_text()
    assert "status: kept" in text
    assert "rating: 5" in text
    assert "done differently" in text


def test_capture_and_daily_link(tmp_path, monkeypatch):
    monkeypatch.setenv("TRANSCRIPT_VAULT", str(tmp_path / "vault"))
    path = vault.save_capture("claude", "hello from the terminal", redact.redact)
    assert path.parent.name == "claude"
    assert "Captures" in path.parts
    vault.add_daily_link(path, "test capture")
    daily = next((tmp_path / "vault").glob("*.md"))
    assert "test capture" in daily.read_text()
