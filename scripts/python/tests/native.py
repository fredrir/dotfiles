import functools
import json
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


@functools.lru_cache(maxsize=1)
def prepared_binaries():
    manifest = os.environ.get("DOTFILE_DEV_BUILD_MANIFEST")
    if not manifest:
        return None
    binaries = {}
    for line in Path(manifest).read_text().splitlines():
        artifact = json.loads(line)
        if artifact.get("reason") == "compiler-artifact" and artifact.get("executable"):
            binaries[artifact["target"]["name"]] = Path(artifact["executable"])
    return binaries


def prepared_binary(name):
    binaries = prepared_binaries()
    if binaries is None:
        return None
    binary = binaries.get(name)
    if binary is None or not binary.is_file():
        raise RuntimeError(f"prepared native binary missing: {name}")
    return binary


def rust_binary(package, name):
    if binary := prepared_binary(name):
        return binary
    result = subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--manifest-path",
            str(ROOT / "scripts/rust/Cargo.toml"),
            "--message-format=json",
            "-p",
            package,
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=180,
    )
    for line in result.stdout.splitlines():
        artifact = json.loads(line)
        if artifact.get("executable") and artifact.get("target", {}).get("name") == name:
            return Path(artifact["executable"])
    raise RuntimeError(f"native binary missing after build: {name}")
