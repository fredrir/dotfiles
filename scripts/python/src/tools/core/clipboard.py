import shutil
import subprocess
import sys


def _read_command():
    if sys.platform == "darwin":
        return ["pbpaste"]
    if shutil.which("wl-paste"):
        return ["wl-paste", "--no-newline", "-t", "text"]
    if shutil.which("xclip"):
        return ["xclip", "-selection", "clipboard", "-o"]
    return None


def _write_command():
    if sys.platform == "darwin":
        return ["pbcopy"]
    if shutil.which("wl-copy"):
        return ["wl-copy"]
    if shutil.which("xclip"):
        return ["xclip", "-selection", "clipboard", "-i"]
    return None


def read_text():
    command = _read_command()
    if command is None:
        return None
    try:
        result = subprocess.run(command, capture_output=True, check=False)
    except OSError:
        return None
    if result.returncode != 0:
        return None
    try:
        return result.stdout.decode("utf-8")
    except UnicodeDecodeError:
        return None


def write_text(text):
    command = _write_command()
    if command is None:
        return False
    try:
        result = subprocess.run(command, input=text.encode("utf-8"), check=False)
    except OSError:
        return False
    return result.returncode == 0
