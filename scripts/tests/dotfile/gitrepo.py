"""Git helpers for the dotfile tests.

Named for what it holds rather than as a second conftest, so that importing it
by name cannot collide with the bench tests' builders.
"""

import subprocess


def run_git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True)
