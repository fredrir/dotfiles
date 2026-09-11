import json
from pathlib import Path

import pytest
from native import prepared_binaries, prepared_binary, rust_binary


@pytest.fixture(autouse=True)
def clear_native_cache(monkeypatch):
    prepared_binaries.cache_clear()
    monkeypatch.delenv("DOTFILE_DEV_BUILD_MANIFEST", raising=False)
    yield
    prepared_binaries.cache_clear()


def test_prepared_binaries_use_cargo_artifacts_without_rebuilding(tmp_path, monkeypatch):
    binary = tmp_path / "custom target" / "dotfile"
    binary.parent.mkdir()
    binary.write_text("fixture")
    binary.chmod(0o755)
    manifest = tmp_path / "artifacts.jsonl"
    manifest.write_text(
        json.dumps(
            {
                "reason": "compiler-artifact",
                "target": {"name": "dotfile"},
                "executable": str(binary),
            }
        )
        + "\n"
        + json.dumps({"reason": "build-finished", "success": True})
    )
    monkeypatch.setenv("DOTFILE_DEV_BUILD_MANIFEST", str(manifest))
    monkeypatch.setenv("PATH", str(tmp_path))
    assert rust_binary("dotfile-cli", "dotfile") == binary
    assert prepared_binary("dotfile") == binary


def test_missing_prepared_binary_fails_instead_of_using_a_stale_executable(tmp_path, monkeypatch):
    manifest = tmp_path / "artifacts.jsonl"
    manifest.write_text("")
    monkeypatch.setenv("DOTFILE_DEV_BUILD_MANIFEST", str(manifest))
    with pytest.raises(RuntimeError, match="prepared native binary missing"):
        rust_binary("dotfile-cli", "dotfile")


def test_direct_pytest_runs_can_build_binaries_in_a_custom_target_directory(tmp_path, monkeypatch):
    binary = tmp_path / "target/debug/demo"

    def build(command, **kwargs):
        assert command[:3] == ["cargo", "build", "--locked"]
        return type(
            "Build",
            (),
            {"stdout": json.dumps({"reason": "compiler-artifact", "target": {"name": "demo"}, "executable": str(binary)})},
        )()

    monkeypatch.setattr("native.subprocess.run", build)
    assert prepared_binaries() is None
    assert rust_binary("demo", "demo") == Path(binary)
