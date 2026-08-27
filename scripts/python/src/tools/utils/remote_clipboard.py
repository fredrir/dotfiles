import subprocess
from typing import Annotated

import typer

from tools.core import clipboard
from tools.core.console import die, out
from tools.core.process import run
from tools.surface import entry as surface

ARCHIE = "archie"
SSH_COMMAND = ["ssh", "-T", "-o", "ConnectTimeout=5", ARCHIE]

REMOTE_SESSION = r"""
runtime=/run/user/$(id -u)
display=$(
  XDG_RUNTIME_DIR="$runtime" systemctl --user show-environment 2>/dev/null |
    sed -n 's/^WAYLAND_DISPLAY=//p'
)
if [ -z "$display" ]; then
  printf '%s\n' 'remote clipboard: no active Wayland session' >&2
  exit 20
fi
export XDG_RUNTIME_DIR="$runtime"
export WAYLAND_DISPLAY="$display"
""".strip()

cpa_app = typer.Typer(add_completion=False)
cpas_app = typer.Typer(add_completion=False)
acp_app = typer.Typer(add_completion=False)


def _remote_copy_command(sensitive):
    option = " --sensitive" if sensitive else ""
    return (
        f"{REMOTE_SESSION}\n"
        "if ! command -v wl-copy >/dev/null 2>&1; then\n"
        "  printf '%s\\n' 'remote clipboard: wl-copy is not installed' >&2\n"
        "  exit 21\n"
        "fi\n"
        f"exec wl-copy --type text/plain{option}"
    )


def _remote_paste_command():
    return (
        f"{REMOTE_SESSION}\n"
        "if ! command -v wl-paste >/dev/null 2>&1; then\n"
        "  printf '%s\\n' 'remote clipboard: wl-paste is not installed' >&2\n"
        "  exit 21\n"
        "fi\n"
        "exec wl-paste --no-newline --type text"
    )


def _text_or_die(prog, text, source):
    if text is None:
        die(prog, f"{source} does not contain readable UTF-8 text")
    if not text.strip():
        die(prog, f"{source} is empty")
    return text


def _remote_failure(prog, result, action):
    detail = result.stderr.decode("utf-8", errors="replace").strip()
    if detail:
        die(prog, detail)
    die(prog, f"could not {action}; ssh exited {result.returncode}")


def _run_ssh(prog, command, **kwargs):
    try:
        return run([*SSH_COMMAND, command], check=False, **kwargs)
    except OSError as error:
        die(prog, f"could not start ssh: {error}")


def send_to_archie(sensitive=False, prog="cpa"):
    text = _text_or_die(prog, clipboard.read_text(), "clipboard")
    result = _run_ssh(
        prog,
        _remote_copy_command(sensitive),
        input=text.encode("utf-8"),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        _remote_failure(prog, result, "copy clipboard to archie")
    suffix = " (sensitive)" if sensitive else ""
    out(f"clipboard → archie{suffix}")


def receive_from_archie(prog="acp"):
    result = _run_ssh(
        prog,
        _remote_paste_command(),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        _remote_failure(prog, result, "read clipboard from archie")
    try:
        text = result.stdout.decode("utf-8")
    except UnicodeDecodeError:
        die(prog, "archie clipboard does not contain readable UTF-8 text")
    text = _text_or_die(prog, text, "archie clipboard")
    if not clipboard.write_text(text):
        die(prog, "could not write the local clipboard")
    out("archie → clipboard")


@cpa_app.command(help="Copy the local text clipboard to Archie.")
def cpa(
    sensitive: Annotated[
        bool,
        typer.Option("-s", "--sensitive", help="Keep the copy out of clipboard history."),
    ] = False,
    completions: str = surface.COMPLETIONS,
):
    send_to_archie(sensitive=sensitive, prog="cpa")


@cpas_app.command(help="Copy the local text clipboard to Archie as sensitive.")
def cpas(completions: str = surface.COMPLETIONS):
    send_to_archie(sensitive=True, prog="cpas")


@acp_app.command(help="Copy Archie's text clipboard to the local clipboard.")
def acp(completions: str = surface.COMPLETIONS):
    receive_from_archie()
