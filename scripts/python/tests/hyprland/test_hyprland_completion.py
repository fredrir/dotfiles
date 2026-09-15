import json


def artifact(path, *, test=False):
    return json.dumps(
        {
            "reason": "compiler-artifact",
            "target": {"name": "dotfile"},
            "profile": {"test": test},
            "executable": str(path),
        }
    )


def executable(path, output):
    path.write_text(f"#!/bin/sh\nprintf '%s\\n' '{output}'\n")
    path.chmod(0o755)


def test_prepared_completion_ignores_path_binary_and_test_harness(tool, tmp_path):
    installed = tmp_path / "dotfile"
    prepared = tmp_path / "prepared"
    harness = tmp_path / "harness"
    executable(installed, "stale installed completion")
    executable(prepared, "fresh prepared completion")
    executable(harness, "test harness")
    manifest = tmp_path / "artifacts.jsonl"
    manifest.write_text(artifact(prepared) + "\n" + artifact(harness, test=True) + "\n")

    for program in ["power-menu", "confirm-exit"]:
        result = tool(
            program,
            "--completions",
            "zsh",
            env={
                "PATH": str(tmp_path),
                "DOTFILE_DEV_BUILD_MANIFEST": str(manifest),
            },
        )
        assert result.returncode == 0, result.stderr
        assert result.stdout == "fresh prepared completion\n"


def test_missing_prepared_completion_does_not_fall_back_to_path(tool, tmp_path):
    installed = tmp_path / "dotfile"
    executable(installed, "stale installed completion")
    manifest = tmp_path / "artifacts.jsonl"
    manifest.write_text(artifact(installed, test=True) + "\n")

    result = tool(
        "power-menu",
        "--completions",
        "zsh",
        env={
            "PATH": str(tmp_path),
            "DOTFILE_DEV_BUILD_MANIFEST": str(manifest),
        },
    )
    assert result.returncode == 1
    assert "prepared native binary missing: dotfile" in result.stderr
    assert not result.stdout
