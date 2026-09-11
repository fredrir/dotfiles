"""Export Python-owned command metadata during setup."""

import argparse
import hashlib
import json
import os
from dataclasses import asdict
from pathlib import Path

from tools.core.paths import repo_root
from tools.surface import entry, zsh


def source_fingerprint(root):
    digest = hashlib.sha256()
    inputs = [root / "scripts/python/pyproject.toml"]
    inputs.extend(sorted((root / "scripts/python/src").rglob("*.py")))
    for path in inputs:
        digest.update(str(path.relative_to(root)).encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def document(root):
    trees = {
        name: entry.introspect.from_typer(entry.load(target), name)
        for name, target in sorted(entry.programs().items())
    }
    return {
        "version": 1,
        "source_fingerprint": source_fingerprint(root),
        "commands": {name: asdict(tree) for name, tree in trees.items()},
        "completions": {name: zsh.script(tree, name) for name, tree in trees.items()},
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    root = Path(repo_root())
    path = root / "config/command-surface.json"
    content = json.dumps(document(root), indent=2, ensure_ascii=False) + "\n"
    previous = path.read_text() if path.exists() else ""
    if content == previous:
        return
    if args.check:
        parser.exit(1, "command metadata is stale; run ./setup.sh --commands-only\n")
    from tempfile import NamedTemporaryFile

    with NamedTemporaryFile(mode="w", dir=path.parent, delete=False, encoding="utf-8") as handle:
        temporary = Path(handle.name)
        try:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
            temporary.chmod(path.stat().st_mode & 0o777 if path.exists() else 0o644)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
