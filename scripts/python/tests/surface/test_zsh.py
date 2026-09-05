import shutil
import subprocess
from pathlib import Path

import pytest

from tools.dotfile.cli import app as dotfile_app
from tools.surface import entry, introspect, zsh
from tools.utils.tardirs import app as tardirs_app

SCRIPT = zsh.script(introspect.from_typer(dotfile_app, "dotfile"))
ROOT = Path(__file__).resolve().parents[4]


def test_the_script_declares_and_registers_the_command():
    assert SCRIPT.startswith("#compdef dotfile\n")
    assert 'if [ "$funcstack[1]" = "_dotfile" ]; then' in SCRIPT
    assert "compdef _dotfile dotfile" in SCRIPT


def test_subcommands_are_offered_with_their_help_text():
    assert (
        "'sync:Refresh generated metadata and reconcile $HOME with the selected profile.'" in SCRIPT
    )
    assert "_dotfile__secret__enroll" in SCRIPT


def test_sync_replaces_the_old_generation_commands():
    assert "'docs:" not in SCRIPT
    assert "'packages:" not in SCRIPT
    assert "--verbose[show every link, merge, generated file, and remote action]" in SCRIPT


def test_a_dispatched_subcommand_is_offered_and_delegated():
    assert "'format:Formats a tree by handing each language to the tool that owns it.'" in SCRIPT
    assert "format) (( $+functions[_dotfile-format] )) && _dotfile-format && ret=0 ;;" in SCRIPT


def test_only_the_root_dispatches():
    assert SCRIPT.count("_dotfile-format") == 2


def test_hidden_commands_stay_out_of_the_offer():
    for hidden in ("'link:", "'profiles:", "'__complete:", "'completions:", "'help:"):
        assert hidden not in SCRIPT


def test_a_constrained_value_is_offered_as_its_choices():
    assert ":resolve:(skip repo live)" in SCRIPT


def test_a_repeatable_option_may_be_given_again():
    assert "'*--override=[" in SCRIPT


def test_an_option_that_takes_no_second_use_rules_itself_out():
    assert "'(--dry-run -n)-n[" in SCRIPT


def test_flags_that_answer_instead_of_running_rule_out_everything_else():
    assert "'(- *)--help[" in SCRIPT
    assert "'(- *)--completions=[" in SCRIPT


def test_a_value_only_the_tool_knows_becomes_a_call_back_into_it():
    assert "_dotfile_complete_profiles" in SCRIPT
    assert "dotfile __complete profiles" in SCRIPT


def test_an_apostrophe_in_help_text_does_not_end_the_quoting():
    assert "Check the profile'\\''s links" in SCRIPT


def test_a_path_argument_completes_paths():
    archives = zsh.script(introspect.from_typer(tardirs_app, "tardirs"))
    assert '_files -g "*.(tar|tgz|tbz2|txz|tar.gz|tar.bz2|tar.xz)"' in archives


@pytest.mark.skipif(not shutil.which("zsh"), reason="zsh is not installed")
@pytest.mark.parametrize("program", sorted(entry.trees()))
def test_zsh_parses_what_was_generated_for(program, tmp_path):
    path = tmp_path / f"{program}.zsh"
    path.write_text(zsh.script(entry.trees()[program], program))
    result = subprocess.run(["zsh", "-n", str(path)], capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stderr


def test_every_installed_tool_lands_in_one_file(tmp_path):
    path, count = entry.write_all(str(tmp_path))
    assert count == len(entry.programs())
    body = (tmp_path / "tools-completion.zsh").read_text()
    assert str(tmp_path) in path
    for program in entry.programs():
        assert f"compdef _{program} {program}" in body


@pytest.mark.skipif(not shutil.which("zsh"), reason="zsh is not installed")
def test_mux_spawns_with_an_exact_hwire_tls_stamp(tmp_path):
    log = tmp_path / "wezterm-arguments"
    script = r"""
WEZTERM_PANE=
source "$HWIRE_WEZTERM_ZSH"
function mux-route { print -- archie-cable }
function wezterm {
  print -r -- "$@" >> "$HWIRE_WEZTERM_LOG"
  [[ $1 == cli && $2 == spawn ]] && print -- 42
  return 0
}
HOST=macie.local
WEZTERM_PANE=7
attach_mux archie
"""
    result = subprocess.run(
        ["zsh", "-f"],
        input=script,
        capture_output=True,
        text=True,
        check=False,
        env={
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "HWIRE_WEZTERM_ZSH": str(ROOT / "shared/zsh/conf.d/49-wezterm.zsh"),
            "HWIRE_WEZTERM_LOG": str(log),
        },
    )
    assert result.returncode == 0, result.stderr
    assert log.read_text().splitlines()[0] == (
        "cli spawn --domain-name archie-cable --cwd /home/fredrir -- "
        "env -i HOME=/home/fredrir TERM=xterm-256color PATH=/usr/local/bin:/usr/bin:/bin "
        "HWIRE_SESSION=v1:macie:archie:cable:tls zsh -l"
    )
