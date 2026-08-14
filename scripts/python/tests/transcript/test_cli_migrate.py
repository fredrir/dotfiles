def test_migrate_previews_and_prompts_before_moving(tool, tmp_path):
    vault = tmp_path / "vault"
    config_file = tmp_path / "config.toml"
    config_file.write_text(
        '[groups]\ndotfiles = ["dotfiles"]\n\n[destinations]\ndotfiles = "Dotfiles/Agents"\n'
    )
    source = vault / "Transcripts/dotfiles/2026-07/codex/17-note.md"
    source.parent.mkdir(parents=True)
    source.write_text("note")
    env = {
        "COLUMNS": "160",
        "TRANSCRIPT_CONFIG": str(config_file),
        "TRANSCRIPT_VAULT": str(vault),
    }

    cancelled = tool("transcript", "migrate", env=env, input_text="\n")
    output = cancelled.stdout + cancelled.stderr
    assert cancelled.returncode == 0
    assert "1 file in 1 group" in output
    assert "dotfiles  1 file" in output
    assert "Transcripts/dotfiles → Dotfiles/Agents" in output
    assert "2026-07/codex  1 file" in output
    assert "17-note.md" not in output
    assert "·" not in output
    assert "[y/N]" in output
    assert "cancelled" in output
    assert source.exists()

    accepted = tool("transcript", "migrate", env=env, input_text="y\n")
    assert accepted.returncode == 0
    assert "moved 1 file" in accepted.stdout + accepted.stderr
    assert not source.exists()
    assert (vault / "Dotfiles/Agents/2026-07/codex/17-note.md").read_text() == "note"


def test_migrate_verbose_lists_relative_files(tool, tmp_path):
    vault = tmp_path / "vault"
    config_file = tmp_path / "config.toml"
    config_file.write_text('[destinations]\ndotfiles = "Dotfiles/Agents"\n')
    source = vault / "Transcripts/dotfiles/2026-07/codex/17-note.md"
    source.parent.mkdir(parents=True)
    source.write_text("note")
    env = {
        "TRANSCRIPT_CONFIG": str(config_file),
        "TRANSCRIPT_VAULT": str(vault),
    }

    result = tool("transcript", "migrate", "--verbose", env=env, input_text="n\n")
    output = result.stdout + result.stderr
    assert result.returncode == 0
    assert "  Files" in output
    assert "    2026-07/codex/17-note.md" in output
    assert "Transcripts/dotfiles/2026-07/codex/17-note.md" not in output


def test_migrate_lists_conflicts_separately(tool, tmp_path):
    vault = tmp_path / "vault"
    config_file = tmp_path / "config.toml"
    config_file.write_text('[destinations]\ndotfiles = "Dotfiles/Agents"\n')
    source = vault / "Transcripts/dotfiles/2026-07/codex/17-note.md"
    destination = vault / "Dotfiles/Agents/2026-07/codex/17-note.md"
    source.parent.mkdir(parents=True)
    destination.parent.mkdir(parents=True)
    source.write_text("new")
    destination.write_text("existing")
    env = {
        "TRANSCRIPT_CONFIG": str(config_file),
        "TRANSCRIPT_VAULT": str(vault),
    }

    result = tool("transcript", "migrate", env=env)
    output = result.stdout + result.stderr
    assert result.returncode == 1
    assert "1 destination conflict" in output
    assert "Dotfiles/Agents/2026-07/codex/17-note.md" in output
    assert "[y/N]" not in output
    assert source.read_text() == "new"
    assert destination.read_text() == "existing"
