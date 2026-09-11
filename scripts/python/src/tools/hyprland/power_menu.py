import typer

from tools.core.process import capture, run
from tools.surface import entry as surface

app = typer.Typer(add_completion=False)

ENTRIES = [
    "\U000f033e  Lock",
    "\U000f0343  Logout",
    "\U000f0904  Suspend",
    "\U000f0453  Reboot",
    "\U000f0425  Shutdown",
]

ACTIONS = {
    "Lock": ["hyprlock"],
    "Logout": ["hyprctl", "dispatch", "exit"],
    "Suspend": ["systemctl", "suspend"],
    "Reboot": ["systemctl", "reboot"],
    "Shutdown": ["systemctl", "poweroff"],
}

WOFI = [
    "wofi",
    "--dmenu",
    "--hide-search",
    "--width",
    "250",
    "--height",
    "260",
    "--location",
    "center",
]


@app.command(help="Show the Hyprland power menu.")
def power_menu(completions: str = surface.COMPLETIONS):
    result = capture(WOFI, input="\n".join(ENTRIES) + "\n")
    fields = result.stdout.split()
    if not fields:
        return
    action = ACTIONS.get(fields[-1])
    if action:
        run(action)
