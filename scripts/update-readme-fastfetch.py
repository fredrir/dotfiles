#!/usr/bin/env python3
"""Regenerate the fastfetch preview block in README.md.

Renders the repo's fastfetch config, converts fastfetch's ANSI cursor-
positioning into real spaces so the columns stay aligned as plain text,
strips colors, drops the Local IP line, and writes the result between the
<!-- fastfetch:start --> / <!-- fastfetch:end --> markers in README.md.

fastfetch detects Shell and Terminal by walking the process tree, so when
this runs from a git hook it would otherwise report "bash"/"git" instead of
the real shell/terminal. Those two values are recomputed from the login
shell and the environment so the preview is correct however it is generated.

Safe to run by hand; also invoked by .githooks/pre-commit when the config
changes. No-ops (exit 0) if fastfetch is unavailable.
"""
import os
import pwd
import re
import shutil
import subprocess
import sys
import unicodedata

ESC = chr(27)
SGR = re.compile(re.escape(ESC) + r"\[[0-9;?]*[A-Za-mo-z]")  # any ANSI escape
COLOR = re.compile(re.escape(ESC) + r"\[[0-9;]*m")  # color/SGR only
CHA = re.compile(re.escape(ESC) + r"\[([0-9]+)G")  # cursor-to-column
VERSION = re.compile(r"\d+(?:\.\d+)+")

START = "<!-- fastfetch:start -->"
END = "<!-- fastfetch:end -->"

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CONFIG = os.path.join(ROOT, "shared", "fastfetch", "config.jsonc")
README = os.path.join(ROOT, "README.md")

DROP = ("Local IP",)


def cell_width(ch: str) -> int:
    if unicodedata.combining(ch):
        return 0
    o = ord(ch)
    # Nerd Font glyphs live in the Private Use Areas and render double-width,
    # matching how fastfetch counts them when it places the value column.
    if 0xE000 <= o <= 0xF8FF or 0xF0000 <= o <= 0xFFFFD or 0x100000 <= o <= 0x10FFFD:
        return 2
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def visible_width(s: str) -> int:
    return sum(cell_width(c) for c in s)


def version_of(*cmd: str) -> str:
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=5).stdout
        m = VERSION.search(out)
        return m.group(0) if m else ""
    except Exception:
        return ""


def real_shell() -> str:
    path = pwd.getpwuid(os.getuid()).pw_shell or os.environ.get("SHELL", "")
    name = os.path.basename(path) or "sh"
    ver = version_of(path or name, "--version")
    return f"{name} {ver}".strip()


def real_terminal() -> str:
    e = os.environ
    if e.get("KONSOLE_VERSION"):
        return f"konsole {version_of('konsole', '--version')}".strip()
    if e.get("KITTY_WINDOW_ID") or e.get("TERM") == "xterm-kitty":
        return f"kitty {version_of('kitty', '--version')}".strip()
    if e.get("ALACRITTY_WINDOW_ID") or e.get("ALACRITTY_SOCKET"):
        return f"alacritty {version_of('alacritty', '--version')}".strip()
    if e.get("WEZTERM_EXECUTABLE") or e.get("TERM_PROGRAM") == "WezTerm":
        return "wezterm"
    if (e.get("TERM") or "").startswith("foot"):
        return f"foot {version_of('foot', '--version')}".strip()
    if e.get("TERM_PROGRAM"):
        return e["TERM_PROGRAM"]
    return e.get("TERM", "terminal")


def override_tail(line: str, value: str) -> str:
    """Replace everything after the final ESC[nG (the value) with `value`."""
    matches = list(CHA.finditer(line))
    if not matches:
        return line
    return line[: matches[-1].end()] + value


def split_line(line: str):
    """Split a rendered line at fastfetch's value cursor-move.

    Returns (prefix, value) with all ANSI removed. `value` is None for lines
    that carry no value (logo art, section headers, title, rule). fastfetch
    marks the value column with `ESC[<n>G`; we use that only as the split
    point and realign the columns ourselves, so glyph-width quirks can't
    misalign the plain-text output.
    """
    line = COLOR.sub("", line)
    matches = list(CHA.finditer(line))
    if not matches:
        return SGR.sub("", line).rstrip(), None
    last = matches[-1]
    prefix = SGR.sub("", line[: last.start()])
    value = SGR.sub("", line[last.end():])
    return prefix, value


def render() -> str:
    proc = subprocess.run(
        ["fastfetch", "--config", CONFIG, "--pipe", "false"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"fastfetch exited {proc.returncode}")

    shell, terminal = real_shell(), real_terminal()
    parsed = []
    for raw in proc.stdout.splitlines():
        plain = SGR.sub("", raw)
        if any(tag in plain for tag in DROP):
            continue
        if re.search(r"\bShell\b", plain):
            raw = override_tail(raw, shell)
        elif re.search(r"\bTerminal\b", plain):
            raw = override_tail(raw, terminal)
        parsed.append(split_line(raw))

    value_col = max((visible_width(p) for p, v in parsed if v is not None), default=0)
    out = []
    for prefix, value in parsed:
        if value is None:
            out.append(prefix.rstrip())
        else:
            pad = " " * max(0, value_col - visible_width(prefix))
            out.append((prefix + pad + value).rstrip())
    while out and not out[0].strip():
        out.pop(0)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


def main() -> int:
    if not shutil.which("fastfetch"):
        print("fastfetch not found; skipping README update")
        return 0

    block = render()
    with open(README, encoding="utf-8") as f:
        text = f.read()

    if START not in text or END not in text:
        raise SystemExit(f"markers {START} / {END} not found in {README}")

    replacement = f"{START}\n\n```\n{block}\n```\n\n{END}"
    new = re.sub(
        re.escape(START) + r".*?" + re.escape(END),
        lambda _m: replacement,
        text,
        flags=re.DOTALL,
    )
    if new != text:
        with open(README, "w", encoding="utf-8") as f:
            f.write(new)
        print("Updated fastfetch preview in README.md")
    else:
        print("fastfetch preview already up to date")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
