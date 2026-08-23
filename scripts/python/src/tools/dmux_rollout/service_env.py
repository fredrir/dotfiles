"""The per-host service environment file (ADR 012 WS-F.1; plan §21 steps 7, 9).

`launchctl setenv` and `systemctl --user set-environment` are runtime-only: a
reboot clears them, and a mux that comes up without the flag is silently
legacy (ADR 012 §3.1 -- Macie lost r5's canary flag exactly that way). The
durable source is one untracked, host-local file:

- macOS: `~/.config/dmux/service.env`, copied into the launchd session at
  login by the `com.fredrir.dmux-env` LaunchAgent and read by
  `dmux-mux-start.sh` itself;
- Linux: `~/.config/environment.d/50-dmux.conf`, read by the systemd user
  manager at session start and on `daemon-reload`.

This module is the rollout tool's copy of the grammar in
`shared/wezterm/mux/dmux-service-env.sh` (`dmux doctor` carries a third, in
Rust); `test_service_env_grammar_matches_the_shell_helper_the_repo_installs`
holds the three in step. The rules, verbatim from the helper:

- a blank line, or a line whose first non-blank character is `#`, is ignored;
- every other line is KEY=VALUE: KEY matches ^DMUX_[A-Z0-9_]*$ and VALUE
  matches ^[A-Za-z0-9_./:@+,-]*$ -- no whitespace, quotes, `$`, backticks,
  `;`, braces, `~`, `\\` or control characters;
- a later assignment to the same KEY wins;
- ONE malformed line refuses the WHOLE file, reported by line number and
  reason, never by content.

Nothing here is ever evaluated, sourced, or handed to a shell.
"""

from __future__ import annotations

import re

from tools.dmux_rollout.errors import Refusal

MAC_RELATIVE_PATH = ".config/dmux/service.env"
LINUX_RELATIVE_PATH = ".config/environment.d/50-dmux.conf"

# Spelled out as the shell helper spells them, so the parity test can compare
# the two literally rather than by interpretation.
KEY_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"
VALUE_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./:@+,-"
KEY_RE = re.compile(r"DMUX_[A-Z0-9_]*")
VALUE_RE = re.compile(r"[A-Za-z0-9_./:@+,-]*")

WEZ_FIRST = "DMUX_WEZ_FIRST"
LEGACY_POLICY = "DMUX_LEGACY_POLICY"


def _line_problem(line: str) -> str | None:
    key, separator, value = line.partition("=")
    if not separator:
        return "expected KEY=VALUE"
    if not key.startswith("DMUX_"):
        return "key must start with DMUX_"
    if not KEY_RE.fullmatch(key):
        return "key must match ^DMUX_[A-Z0-9_]*$"
    if not VALUE_RE.fullmatch(value):
        return "value must match ^[A-Za-z0-9_./:@+,-]*$ (no whitespace, quotes, $, backticks or ;)"
    return None


def _split(text: str) -> list[str]:
    # Split on "\n" only: the helper reads with `read -r`, so a "\r" stays in
    # the value and is refused there; splitlines() would hide it.
    return text.split("\n")


def _is_assignment(raw: str) -> bool:
    line = raw.lstrip()
    return bool(line) and not line.startswith("#")


def parse(text: str, *, name: str) -> dict[str, str]:
    """The validated assignments of `text`, last assignment winning.

    Raises `Refusal` naming every bad line by number and reason when any line
    is malformed; nothing is applied from such a file.
    """
    assignments: dict[str, str] = {}
    problems: list[str] = []
    for number, raw in enumerate(_split(text), 1):
        if not _is_assignment(raw):
            continue
        line = raw.lstrip()
        problem = _line_problem(line)
        if problem is not None:
            problems.append(f"line {number}: {problem}")
            continue
        key, _, value = line.partition("=")
        assignments[key] = value
    if problems:
        raise Refusal(f"{name} is malformed and applies nothing ({'; '.join(problems)})")
    return assignments


def require_assignment(key: str, value: str) -> None:
    if not KEY_RE.fullmatch(key):
        raise Refusal(f"service environment key {key!r} does not match ^DMUX_[A-Z0-9_]*$")
    if not VALUE_RE.fullmatch(value):
        raise Refusal(f"service environment value for {key} does not match the file grammar")


def render(text: str, assignments: dict[str, str], *, name: str) -> str:
    """`text` with `assignments` applied, every other line kept verbatim.

    Existing assignments to the given keys are dropped and the new ones are
    appended, which is "last assignment wins" made explicit. The input must
    already be well-formed: a file this tool cannot vouch for is never
    rewritten, only reported.
    """
    parse(text, name=name)
    for key, value in assignments.items():
        require_assignment(key, value)
    kept = [
        raw
        for raw in _split(text)
        if not (_is_assignment(raw) and raw.lstrip().partition("=")[0] in assignments)
    ]
    while kept and kept[-1] == "":
        kept.pop()
    kept.extend(f"{key}={value}" for key, value in assignments.items())
    return "\n".join(kept) + "\n"


def without(text: str, keys: set[str], *, name: str) -> str:
    """`text` with every assignment to `keys` removed, everything else verbatim."""
    parse(text, name=name)
    kept = [
        raw
        for raw in _split(text)
        if not (_is_assignment(raw) and raw.lstrip().partition("=")[0] in keys)
    ]
    while kept and kept[-1] == "":
        kept.pop()
    return "\n".join(kept) + "\n" if kept else ""
