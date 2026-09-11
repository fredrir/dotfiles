"""Resolve the native executable built for this checkout."""

import json
import os
import shutil
from pathlib import Path

from tools.core.paths import repo_root


def binary(name):
    manifest = os.environ.get("DOTFILE_DEV_BUILD_MANIFEST")
    if manifest:
        selected = None
        for line in Path(manifest).read_text().splitlines():
            artifact = json.loads(line)
            if (
                artifact.get("reason") == "compiler-artifact"
                and artifact.get("target", {}).get("name") == name
                and artifact.get("executable")
            ):
                selected = Path(artifact["executable"])
        if selected and selected.is_file() and os.access(selected, os.X_OK):
            return str(selected)
        raise RuntimeError(f"prepared native binary missing: {name}")
    root = Path(repo_root())
    candidates = [
        root / "scripts/rust/target/debug" / name,
        root / "scripts/rust/target/release" / name,
        Path.home() / ".local/bin" / name,
    ]
    available = [path for path in candidates if path.is_file() and os.access(path, os.X_OK)]
    if available:
        return str(max(available, key=lambda path: path.stat().st_mtime))
    if found := shutil.which(name):
        return found
    raise RuntimeError(f"{name} is not built; run ./setup.sh")
