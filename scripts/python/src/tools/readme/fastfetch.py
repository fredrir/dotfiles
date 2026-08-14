import os
import pwd
import re
import shutil
import subprocess
import sys
import unicodedata

import typer

from tools.core.paths import repo_root

app = typer.Typer(add_completion=False)

ESC = chr(27)
ANY_ESCAPE = re.compile(re.escape(ESC) + r"\[[0-9;?]*[A-Za-mo-z]")
COLOR_ESCAPE = re.compile(re.escape(ESC) + r"\[[0-9;]*m")
COLUMN_ESCAPE = re.compile(re.escape(ESC) + r"\[([0-9]+)G")
VERSION = re.compile(r"\d+(?:\.\d+)+")

START = "<!-- fastfetch:start -->"
END = "<!-- fastfetch:end -->"

DROPPED_ROWS = ("Local IP",)

PRIVATE_USE_RANGES = (
    (0xE000, 0xF8FF),
    (0xF0000, 0xFFFFD),
    (0x100000, 0x10FFFD),
)


def config_path():
    return os.path.join(repo_root(), "shared", "fastfetch", "config.jsonc")


def readme_path():
    return os.path.join(repo_root(), "README.md")


def cell_width(char):
    if unicodedata.combining(char):
        return 0
    point = ord(char)
    for low, high in PRIVATE_USE_RANGES:
        if low <= point <= high:
            return 2
    return 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1


def visible_width(text):
    return sum(cell_width(char) for char in text)


def version_of(*command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=5, check=False)
        match = VERSION.search(result.stdout)
        return match.group(0) if match else ""
    except Exception:
        return ""


def real_shell():
    shell = pwd.getpwuid(os.getuid()).pw_shell or os.environ.get("SHELL", "")
    name = os.path.basename(shell) or "sh"
    return f"{name} {version_of(shell or name, '--version')}".strip()


def real_terminal():
    env = os.environ
    if env.get("KONSOLE_VERSION"):
        return f"konsole {version_of('konsole', '--version')}".strip()
    if env.get("KITTY_WINDOW_ID") or env.get("TERM") == "xterm-kitty":
        return f"kitty {version_of('kitty', '--version')}".strip()
    if env.get("ALACRITTY_WINDOW_ID") or env.get("ALACRITTY_SOCKET"):
        return f"alacritty {version_of('alacritty', '--version')}".strip()
    if (env.get("TERM") or "").startswith("foot"):
        return f"foot {version_of('foot', '--version')}".strip()
    if env.get("TERM_PROGRAM"):
        return env["TERM_PROGRAM"]
    return env.get("TERM", "terminal")


def override_value(line, value):
    matches = list(COLUMN_ESCAPE.finditer(line))
    if not matches:
        return line
    return line[: matches[-1].end()] + value


def split_row(line):
    line = COLOR_ESCAPE.sub("", line)
    matches = list(COLUMN_ESCAPE.finditer(line))
    if not matches:
        return ANY_ESCAPE.sub("", line).rstrip(), None
    last = matches[-1]
    label = ANY_ESCAPE.sub("", line[: last.start()])
    value = ANY_ESCAPE.sub("", line[last.end() :])
    return label, value


def render():
    result = subprocess.run(
        ["fastfetch", "--config", config_path(), "--pipe", "false"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(f"fastfetch exited {result.returncode}")

    shell, terminal = real_shell(), real_terminal()
    rows = []
    for raw in result.stdout.splitlines():
        plain = ANY_ESCAPE.sub("", raw)
        if any(tag in plain for tag in DROPPED_ROWS):
            continue
        if re.search(r"\bShell\b", plain):
            raw = override_value(raw, shell)
        elif re.search(r"\bTerminal\b", plain):
            raw = override_value(raw, terminal)
        rows.append(split_row(raw))

    column = max((visible_width(label) for label, value in rows if value is not None), default=0)
    lines = []
    for label, value in rows:
        if value is None:
            lines.append(label.rstrip())
        else:
            padding = " " * max(0, column - visible_width(label))
            lines.append((label + padding + value).rstrip())
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return "\n".join(lines)


@app.command(help="Refresh the fastfetch preview block in README.md.")
def update():
    if not shutil.which("fastfetch"):
        print("fastfetch not found; skipping README update")
        return

    readme = readme_path()
    block = render()
    with open(readme, encoding="utf-8") as handle:
        text = handle.read()

    if START not in text or END not in text:
        raise SystemExit(f"markers {START} / {END} not found in {readme}")

    replacement = f"{START}\n\n```\n{block}\n```\n\n{END}"
    updated = re.sub(
        re.escape(START) + r".*?" + re.escape(END),
        lambda _match: replacement,
        text,
        flags=re.DOTALL,
    )
    if updated != text:
        with open(readme, "w", encoding="utf-8") as handle:
            handle.write(updated)
        print("Updated fastfetch preview in README.md")
    else:
        print("fastfetch preview already up to date")
