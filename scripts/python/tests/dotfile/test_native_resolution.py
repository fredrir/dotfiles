import json

import native
import pytest

from tools.transcript.native import binary


@pytest.mark.parametrize("resolve", [binary, native.prepared_binary])
def test_prepared_binary_excludes_test_harnesses(resolve, tmp_path, monkeypatch):
    prepared = tmp_path / "prepared"
    harness = tmp_path / "harness"
    for path in [prepared, harness]:
        path.write_text("#!/bin/sh\nexit 0\n")
        path.chmod(0o755)
    manifest = tmp_path / "artifacts.jsonl"
    manifest.write_text("\n".join(json.dumps({
        "reason": "compiler-artifact",
        "target": {"name": "dotfile"},
        "profile": {"test": test},
        "executable": str(path),
    }) for path, test in [(prepared, False), (harness, True)]))
    monkeypatch.setenv("DOTFILE_DEV_BUILD_MANIFEST", str(manifest))
    native.prepared_binaries.cache_clear()
    try:
        assert str(resolve("dotfile")) == str(prepared)
        prepared.unlink()
        with pytest.raises(RuntimeError, match="prepared native binary missing: dotfile"):
            resolve("dotfile")
    finally:
        native.prepared_binaries.cache_clear()
