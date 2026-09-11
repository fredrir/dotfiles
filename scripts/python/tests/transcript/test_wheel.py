import json
import os
import subprocess
import sys
import zipfile

from native import ROOT, rust_binary


def test_wheel_installs_only_retained_tools_and_runs_outside_repository(tmp_path):
    project = ROOT / "scripts/python"

    def run(*arguments, env=None):
        result = subprocess.run(
            arguments,
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        return result.stdout

    run("uv", "build", "--offline", "--wheel", "--out-dir", str(tmp_path), str(project))
    wheel = next(tmp_path.glob("*.whl"))
    with zipfile.ZipFile(wheel) as archive:
        packages = {
            path.split("/")[1]
            for path in archive.namelist()
            if path.startswith("tools/") and path.count("/") > 1
        }
    assert packages == {"hyprland", "transcript"}
    requirements = tmp_path / "pylock.toml"
    requirements.write_text(
        run("uv", "export", "--project", str(project), "--locked", "--offline", "--no-dev",
            "--no-emit-project", "--no-header", "--format", "pylock.toml")
    )
    environment = tmp_path / "environment"
    run("uv", "venv", "--offline", "--python", sys.executable, str(environment))
    python = environment / "bin/python"
    run("uv", "pip", "sync", "--offline", "--python", str(python), str(requirements))
    run("uv", "pip", "install", "--offline", "--no-deps", "--python", str(python), str(wheel))
    native = rust_binary("dotfile-cli", "dotfile")
    env = dict(os.environ, HOME=str(tmp_path), PATH=f"{native.parent}:/usr/bin:/bin")
    env.pop("PYTHONPATH", None)
    env.pop("DOTFILE_ROOT", None)
    for program in ["transcript", "clean-copy", "power-menu", "confirm-exit"]:
        assert "Usage:" in run(str(environment / "bin" / program), "--help", env=env)
    script = "import json, tools.transcript, tools.hyprland; print(json.dumps([tools.transcript.__file__, tools.hyprland.__file__]))"
    imports = json.loads(run(str(python), "-I", "-c", script, env=env))
    assert all(path.startswith(str(environment)) for path in imports)
    completion = run(str(environment / "bin/transcript"), "--completions", "zsh", env=env)
    assert "#compdef transcript" in completion
