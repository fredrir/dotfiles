import os
import shutil
import subprocess
import tomllib

import pytest

from tools.core.paths import repo_root
from tools.dotfile import jsonc

REPO = str(repo_root())
TAPLO = os.path.join(REPO, "shared/tools/.taplo.toml")
SETTINGS = os.path.join(REPO, "shared/vscode/settings.json")

PREFIX = "evenBetterToml.formatter."


PINNED = ("indentString",)

needs_taplo = pytest.mark.skipif(not shutil.which("taplo"), reason="taplo is not installed")


def camel(key):
    head, *rest = key.split("_")
    return head + "".join(word.capitalize() for word in rest)


def formatting():
    with open(TAPLO, "rb") as handle:
        return tomllib.load(handle).get("formatting", {})


def settings():
    with open(SETTINGS, encoding="utf-8") as handle:
        return jsonc.loads(handle.read())


def literal(value):
    return ("true" if value else "false") if isinstance(value, bool) else str(value)


def complaint(text):
    """taplo's own message, picked out of the INFO lines it logs beside it."""
    lines = [line for line in text.splitlines() if "error" in line.lower()]
    return (lines[-1] if lines else text).strip()


def probe(tmp_path, option):
    target = tmp_path / "probe.toml"
    target.write_text("a = 1\n")
    # --no-auto-config so a .taplo.toml found by searching upward cannot answer
    # in place of the option under test.
    return subprocess.run(
        ["taplo", "fmt", "--no-auto-config", "-o", option, str(target)],
        capture_output=True,
        text=True,
        check=False,
    )


def test_every_taplo_setting_is_mirrored_in_the_editor():
    found = settings()
    for key, value in formatting().items():
        name = PREFIX + camel(key)
        assert name in found, (
            f"shared/tools/.taplo.toml sets {key}, and shared/vscode/settings.json "
            f"has no {name}. VS Code would format TOML differently from "
            f"`dotfile format`; if the editor cannot express {key}, that is the "
            "thing to resolve rather than to leave unsaid."
        )
        assert found[name] == value, (
            f"{name} is {found[name]!r} and shared/tools/.taplo.toml says {key} = {value!r}"
        )


def test_every_editor_setting_is_mirrored_or_deliberately_pinned():
    mirrored = {PREFIX + camel(key) for key in formatting()}
    for name in settings():
        if not name.startswith(PREFIX) or name in mirrored:
            continue
        assert name[len(PREFIX) :] in PINNED, (
            f"shared/vscode/settings.json sets {name}, which shared/tools/.taplo.toml "
            "does not. Put it in that file so the CLI agrees, or in PINNED here with "
            "the reason it is deliberately editor-only."
        )


@needs_taplo
def test_every_taplo_setting_is_an_option_taplo_knows(tmp_path):
    for key, value in formatting().items():
        result = probe(tmp_path, f"{key}={literal(value)}")
        assert result.returncode == 0, (
            f"shared/tools/.taplo.toml sets {key}, which taplo does not accept: "
            f"{complaint(result.stderr)}"
        )


@needs_taplo
def test_taplo_still_refuses_an_option_it_does_not_know(tmp_path):
    # The check above is only worth having while `-o` validates. taplo's
    # configuration file reader does not, so if this ever passes, a typo in
    # .taplo.toml has become undetectable and the guard has gone blind.
    assert probe(tmp_path, "not_a_real_option=true").returncode != 0
