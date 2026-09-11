import json
import os
import shutil
from pathlib import Path


def binary(name):
    manifest = os.environ.get("DOTFILE_DEV_BUILD_MANIFEST")
    if manifest:
        selected = None
        for line in Path(manifest).read_text().splitlines():
            artifact = json.loads(line)
            if (
                artifact.get("reason") == "compiler-artifact"
                and not artifact.get("profile", {}).get("test", False)
                and artifact.get("target", {}).get("name") == name
                and artifact.get("executable")
            ):
                selected = Path(artifact["executable"])
        if selected and selected.is_file() and os.access(selected, os.X_OK):
            return str(selected)
        raise RuntimeError(f"prepared native binary missing: {name}")
    if found := shutil.which(name):
        return found
    raise RuntimeError(f"{name} not found; run ./setup.sh")
