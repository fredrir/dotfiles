"""Authored reference text shared with the native documentation renderer."""

import json
from pathlib import Path

from tools.core.paths import repo_root

CATALOG = json.loads(
    (Path(repo_root()) / "scripts/rust/crates/dotfile/assets/cli-reference.json").read_text()
)
