import typer

from tools.core.process import capture, run

app = typer.Typer(add_completion=False)

WOFI = [
    "wofi",
    "--dmenu",
    "--prompt",
    "Exit Hyprland?",
    "--width",
    "200",
    "--height",
    "120",
]


@app.command(help="Ask for confirmation, then exit Hyprland.")
def confirm_exit():
    result = capture(WOFI, input="Yes\nNo\n")
    if "Yes" in result.stdout:
        run(["hyprctl", "dispatch", "exit"])
