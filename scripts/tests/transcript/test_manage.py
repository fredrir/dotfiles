from tools.transcript import config, manage


def use_config(tmp_path, monkeypatch, text=""):
    path = tmp_path / "config.toml"
    if text:
        path.write_text(text)
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(path))
    return path


def test_track_adds_project(tmp_path, monkeypatch):
    use_config(tmp_path, monkeypatch)
    repo = tmp_path / "openclaw"
    repo.mkdir()
    project, added = manage.track(repo)
    assert project == "openclaw"
    assert added
    assert "openclaw" in config.allowed_projects()
    _, added_again = manage.track(repo)
    assert not added_again


def test_track_with_name_creates_alias_and_group(tmp_path, monkeypatch):
    use_config(tmp_path, monkeypatch)
    repo = tmp_path / "llunde-new" / "backend"
    repo.mkdir(parents=True)
    project, added = manage.track(repo, name="llunde-backend", group="llunde")
    assert project == "llunde-backend"
    assert added
    assert config.project_aliases()[("llunde-new", "backend")] == "llunde-backend"
    assert "llunde-backend" in config.project_groups()["llunde"]
    assert "llunde-backend" in config.allowed_projects()


def test_track_preserves_existing_entries(tmp_path, monkeypatch):
    path = use_config(tmp_path, monkeypatch, 'projects = ["dotfiles"]\nmin_rounds = 2\n')
    repo = tmp_path / "sndbx"
    repo.mkdir()
    manage.track(repo)
    text = path.read_text()
    assert "dotfiles" in text
    assert "min_rounds = 2" in text
    assert config.allowed_projects() == {"dotfiles", "sndbx"}


def test_untrack_removes_project_alias_and_group(tmp_path, monkeypatch):
    use_config(
        tmp_path,
        monkeypatch,
        'projects = ["dotfiles", "llunde-backend"]\n'
        "\n"
        "[aliases]\n"
        '"llunde-new/backend" = "llunde-backend"\n'
        "\n"
        "[groups]\n"
        'llunde = ["llunde-backend"]\n',
    )
    assert manage.untrack("llunde-backend")
    assert config.allowed_projects() == {"dotfiles"}
    assert config.project_aliases() == {}
    assert config.project_groups() == {}


def test_untrack_unknown_project_returns_false(tmp_path, monkeypatch):
    use_config(tmp_path, monkeypatch, 'projects = ["dotfiles"]\n')
    assert not manage.untrack("missing")
    assert config.allowed_projects() == {"dotfiles"}
