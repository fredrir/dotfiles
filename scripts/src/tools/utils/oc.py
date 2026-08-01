import os

import typer

from tools.core.process import capture, run

app = typer.Typer(add_completion=False)

TUNNEL_PORT = 18789
TUNNEL_HOST = "hetzner"


def tunnel_listening():
    try:
        result = capture(["ss", "-Htln", f"sport = :{TUNNEL_PORT}"])
    except FileNotFoundError:
        return False
    return bool(result.stdout.strip())


def ensure_tunnel():
    if tunnel_listening():
        return
    run(
        [
            "ssh",
            "-f",
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-L",
            f"{TUNNEL_PORT}:127.0.0.1:{TUNNEL_PORT}",
            TUNNEL_HOST,
        ]
    )


@app.command(help="Open the openclaw TUI, or send a message, over the SSH tunnel.")
def oc(message: list[str] | None = typer.Argument(None)):
    ensure_tunnel()
    if message:
        os.execvp(
            "openclaw",
            ["openclaw", "agent", "--agent", "main", "--message", " ".join(message)],
        )
    else:
        os.execvp("openclaw", ["openclaw", "tui"])
