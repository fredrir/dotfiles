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

    cancelled = tool("transcript", "migrate", env=env, input_text="n\n")
    output = cancelled.stdout + cancelled.stderr
    assert cancelled.returncode == 0
    assert "Transcripts/dotfiles/2026-07/codex/17-note.md" in output
    assert "Dotfiles/Agents/2026-07/codex/17-note.md" in output
    assert "[Y/n]" in output
    assert "cancelled" in output
    assert source.exists()

    accepted = tool("transcript", "migrate", env=env, input_text="\n")
    assert accepted.returncode == 0
    assert "moved 1 file" in accepted.stdout + accepted.stderr
    assert not source.exists()
    assert (vault / "Dotfiles/Agents/2026-07/codex/17-note.md").read_text() == "note"
