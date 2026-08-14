import pytest

from tools.transcript import config


@pytest.fixture(autouse=True)
def isolated_transcript_config(tmp_path, monkeypatch):
    path = tmp_path / "default-transcript-config.toml"
    path.write_text("")
    monkeypatch.setenv("TRANSCRIPT_CONFIG", str(path))
    config._load_file.cache_clear()
    yield
    config._load_file.cache_clear()
