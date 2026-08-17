from __future__ import annotations

import hashlib
import os
import re
import shlex
import subprocess
import sys
from collections.abc import Mapping, Sequence
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path

from tools.dmux_rollout.errors import CommandError


@dataclass(frozen=True)
class Result:
    argv: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str


class Runner:
    def capture(
        self,
        argv: Sequence[str],
        *,
        cwd: Path | None = None,
        env: Mapping[str, str] | None = None,
        unset_env: Sequence[str] = (),
        timeout: float = 120,
        check: bool = True,
    ) -> Result:
        command = tuple(str(part) for part in argv)
        try:
            completed = subprocess.run(
                command,
                cwd=cwd,
                env=self._environment(env, unset_env),
                capture_output=True,
                text=True,
                timeout=timeout,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise CommandError(f"could not run {shlex.join(command)}: {error}") from error
        result = Result(command, completed.returncode, completed.stdout, completed.stderr)
        if check and completed.returncode != 0:
            detail = (completed.stderr or completed.stdout).strip()
            raise CommandError(f"{shlex.join(command)} exited {completed.returncode}: {detail}")
        return result

    def stream(
        self,
        argv: Sequence[str],
        *,
        cwd: Path | None = None,
        env: Mapping[str, str] | None = None,
        unset_env: Sequence[str] = (),
        log: Path | None = None,
        timeout: float | None = None,
    ) -> Result:
        command = tuple(str(part) for part in argv)
        output = []
        with ExitStack() as stack:
            log_handle = None
            if log is not None:
                log.parent.mkdir(parents=True, exist_ok=True)
                log_handle = stack.enter_context(open(log, "a", encoding="utf-8"))
                os.chmod(log, 0o600)
            try:
                process = subprocess.Popen(
                    command,
                    cwd=cwd,
                    env=self._environment(env, unset_env),
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    bufsize=1,
                )
            except OSError as error:
                raise CommandError(f"could not run {shlex.join(command)}: {error}") from error
            assert process.stdout is not None
            try:
                for line in process.stdout:
                    output.append(line)
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    if log_handle is not None:
                        log_handle.write(line)
                        log_handle.flush()
                returncode = process.wait(timeout=timeout)
            except BaseException:
                process.terminate()
                process.wait(timeout=10)
                raise
            text = "".join(output)
            result = Result(command, returncode, text, "")
            if returncode != 0:
                raise CommandError(f"{shlex.join(command)} exited {returncode}")
            if log_handle is not None:
                os.fsync(log_handle.fileno())
            return result

    @staticmethod
    def _environment(extra: Mapping[str, str] | None, unset: Sequence[str] = ()) -> dict[str, str]:
        environment = dict(os.environ)
        for name in unset:
            environment.pop(name, None)
        if extra:
            environment.update({str(key): str(value) for key, value in extra.items()})
        return environment


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def remote_argv(host: str, argv: Sequence[str]) -> list[str]:
    if not re.fullmatch(r"[A-Za-z0-9_.@:-]+", host):
        raise CommandError("SSH host must be a nonempty token")
    return ["ssh", "-o", "BatchMode=yes", host, "--", shlex.join([str(part) for part in argv])]
