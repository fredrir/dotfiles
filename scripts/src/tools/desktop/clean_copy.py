import re
import subprocess
import sys
from textwrap import dedent
from typing import Annotated

import typer

app = typer.Typer(add_completion=False)

ESCAPES = re.compile(
    "\x1b\\[[0-9;?]*[ -/]*[@-~]|\x1b\\][^\x07\x1b]*(?:\x07|\x1b\\\\)|\x1b[@-Z\\\\^_]"
)

CONTROL_CHARS = {code: None for code in range(0x20) if chr(code) not in "\n\t"}
CONTROL_CHARS[0x7F] = None

ZERO_WIDTH = dict.fromkeys(map(ord, "\u200b\u200c\u200d\u2060\ufeff"), None)
SPACE_LIKE = dict.fromkeys(map(ord, "\u00a0\u202f\u2007"), " ")

TRANSLATIONS = {**CONTROL_CHARS, **ZERO_WIDTH, **SPACE_LIKE}


def clean_text(text):
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = ESCAPES.sub("", text)
    text = text.translate(TRANSLATIONS)
    lines = [line.rstrip() for line in text.split("\n")]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return dedent("\n".join(lines))


def read_clipboard():
    try:
        result = subprocess.run(
            ["wl-paste", "--no-newline", "-t", "text"],
            capture_output=True,
            check=False,
        )
    except FileNotFoundError:
        return None
    if result.returncode != 0:
        return None
    try:
        return result.stdout.decode("utf-8")
    except UnicodeDecodeError:
        return None


def write_clipboard(text):
    subprocess.run(["wl-copy"], input=text.encode("utf-8"), check=False)


@app.command(help="Clean selected text and write it to the clipboard.")
def clean_copy(
    stdin: Annotated[bool, typer.Option("--stdin", help="Read selected text from stdin.")] = False,
):
    text = sys.stdin.read() if stdin else read_clipboard()
    if text is None:
        return
    cleaned = clean_text(text)
    if not cleaned:
        return
    write_clipboard(cleaned)
