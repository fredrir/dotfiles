"""Workspace actions; tmux owns navigation and status rendering."""

import argparse
import concurrent.futures
import contextlib
import fcntl
import hashlib
import json
import os
import re
import select
import shlex
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import termios
import time
import tty
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXECUTABLE = HERE / "tmux-workspace"
SHELF = "__workspace-shelf"
SEP = "\t"
SHELLS = {"zsh", "bash", "sh", "fish", "dash", "ksh"}


class Error(Exception):
    pass


def clean(value, limit=160):
    return "".join(c for c in str(value) if c.isprintable())[:limit]


def identifier(value):
    return hashlib.sha256(str(value).encode()).hexdigest()[:12]


def version(value):
    match = re.search(r"(\d+)\.(\d+)", value)
    return tuple(map(int, match.groups())) if match else (0, 0)


def call(
    argv, *, check=True, capture=True, input=None, cwd=None, env=None, timeout=None
):
    try:
        result = subprocess.run(
            [str(arg) for arg in argv],
            text=True,
            input=input,
            cwd=cwd,
            env=env,
            capture_output=capture,
            timeout=timeout,
            check=False,
        )
    except FileNotFoundError as exc:
        raise Error(f"{argv[0]}: not installed") from exc
    except subprocess.TimeoutExpired as exc:
        raise Error(f"{argv[0]}: timed out") from exc
    if check and result.returncode:
        raise Error(
            clean(result.stderr or f"{argv[0]} exited {result.returncode}", 600)
        )
    return result


def json_write(path, value):
    path.write_text(json.dumps(value), encoding="utf-8")
    path.chmod(0o600)


def config_home():
    return Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "tmux"


def session_base(path):
    return (
        re.sub(r"[^a-zA-Z0-9_-]+", "-", Path(path).name).strip("-")[:45] or "workspace"
    )


def terminal_marker(fd, enabled):
    os.write(
        fd,
        b"\033]1337;SetUserVar=TMUX_WORKSPACE="
        + (b"MQ==" if enabled else b"")
        + b"\007",
    )


class Workspace:
    def __init__(self, args):
        self.args = args
        self.pane = args.pane or os.environ.get("TMUX_PANE", "")
        self.client = args.client or ""
        self.socket = args.socket or os.environ.get("TMUX_WORKSPACE_SOCKET", "")
        self.tmux = ["tmux"] + (["-S", self.socket] if self.socket else [])
        self._version = None

    def run(self, *args, **kwargs):
        return call(self.tmux + list(args), **kwargs)

    def fmt(self, expression, pane=None, client=None, check=True):
        target = pane or self.pane
        cmd = ["display-message", "-p"]
        if target:
            cmd += ["-t", target]
        if client or self.client:
            cmd += ["-c", client or self.client]
        return self.run(*cmd, expression, check=check).stdout.strip()

    @property
    def server_version(self):
        if self._version is None:
            found = self.fmt("#{version}", check=False)
            self._version = version(found or self.run("-V").stdout)
        return self._version

    def context(self):
        if not self.pane:
            self.pane = self.fmt("#{pane_id}")
        if not re.fullmatch(r"%\d+", self.pane):
            raise Error("tmux pane required")
        if not self.client:
            current_session = self.fmt("#{session_name}")
            rows = self.client_rows()
            internal_pane = self.fmt("#{@workspace-internal}")
            clients = [
                row["name"]
                for row in rows
                if (internal_pane or not row["internal"])
                and row["session"] == current_session
            ]
            if len(clients) == 1:
                self.client = clients[0]
        return self

    def cwd(self):
        value = self.fmt("#{pane_current_path}", check=False) if self.pane else ""
        return Path(value) if value and Path(value).is_dir() else Path.cwd()

    def root(self):
        cwd = self.cwd()
        if shutil.which("git"):
            result = call(
                ["git", "-C", cwd, "rev-parse", "--show-toplevel"], check=False
            )
            if result.returncode == 0 and result.stdout.strip():
                return Path(result.stdout.strip())
        return cwd

    def notice(self, message):
        if self.pane or self.client:
            cmd = ["display-message", "-d", "4500"]
            if self.client:
                cmd += ["-c", self.client]
            self.run(*cmd, "--", clean(message, 600).replace("#", "##"), check=False)
        else:
            print(message)

    def self_command(self, command, *extra):
        result = [str(EXECUTABLE), command]
        if self.pane:
            result += ["--pane", self.pane]
        if self.client:
            result += ["--client", self.client]
        if self.socket:
            result += ["--socket", self.socket]
        return result + list(map(str, extra))

    def popup(self, argv, title, cwd=None, close_on_exit=True):
        self.context()
        if not self.client:
            raise Error("attached client required; run from an attached tmux terminal")
        if self.server_version >= (3, 2):
            result = self.run(
                "display-popup",
                *(["-E"] if close_on_exit else []),
                "-c",
                self.client,
                "-d",
                cwd or self.cwd(),
                "-w",
                "88%",
                "-h",
                "82%",
                "-T",
                f" {clean(title)} ",
                shlex.join(list(map(str, argv))),
                check=close_on_exit,
            )
            if not close_on_exit and result.returncode not in (0, 129, 130):
                raise Error(clean(result.stderr or f"popup exited {result.returncode}"))
            return
        # The helper's private result file synchronizes older tmux split views.
        self.run(
            "split-window",
            "-t",
            self.pane,
            "-l",
            "70%",
            "-c",
            cwd or self.cwd(),
            shlex.join(list(map(str, argv))),
        )

    def choose(self, rows, title, *, header="Enter open · Esc cancel"):
        if not rows:
            self.notice(f"{title}: empty")
            return None
        with tempfile.TemporaryDirectory(prefix="tmux-workspace-") as directory:
            directory = Path(directory)
            data, output, done = (
                directory / name for name in ("choices.json", "choice.json", "done")
            )
            colors = self.run(
                "show-options", "-gqv", "@theme_fzf_colors", check=False
            ).stdout.strip()
            json_write(
                data, {"rows": rows, "title": title, "header": header, "colors": colors}
            )
            command = self.self_command(
                "_pick", "--data", data, "--result", output, "--done", done
            )
            if sys.stdin.isatty() and not self.pane:
                call(command, capture=False, check=False)
            else:
                self.popup(command, title)
                if self.server_version < (3, 2):
                    while not done.exists():
                        if self.run("has-session", check=False).returncode:
                            return None
                        time.sleep(0.1)
            if not output.exists():
                return None
            try:
                index = json.loads(output.read_text())
            except (OSError, ValueError):
                return None
            return (
                rows[index]
                if isinstance(index, int) and 0 <= index < len(rows)
                else None
            )

    def pick(self):
        try:
            data = json.loads(Path(self.args.data).read_text())
            rows = data["rows"]
            lines = [
                f"{index}\t{clean(row['label'], 1000)}"
                for index, row in enumerate(rows)
            ]
            selected = None
            if shutil.which("fzf"):
                env = dict(os.environ, FZF_DEFAULT_OPTS="", FZF_DEFAULT_OPTS_FILE="")
                options = [
                    "fzf",
                    "--layout=reverse",
                    "--border=rounded",
                    "--no-multi",
                    "--delimiter=\t",
                    "--with-nth=2..",
                    "--prompt",
                    f"{data['title']} › ",
                    "--header",
                    data["header"],
                    "--cycle",
                ]
                if data.get("colors"):
                    options += ["--color", data["colors"]]
                result = call(
                    options,
                    input="\n".join(lines),
                    env=env,
                    check=False,
                )
                if result.returncode == 0:
                    selected = int(result.stdout.partition("\t")[0])
            else:
                print(
                    f"{data['title']} — fzf unavailable; enter a number or search text\n"
                )
                remaining = list(enumerate(rows))
                while remaining:
                    for index, row in remaining[:80]:
                        print(f"{index + 1:>3}  {clean(row['label'], 180)}")
                    choice = input("Number / search / empty to cancel › ").strip()
                    if not choice:
                        break
                    if choice.isdigit() and 1 <= int(choice) <= len(rows):
                        selected = int(choice) - 1
                        break
                    remaining = [
                        (i, row)
                        for i, row in enumerate(rows)
                        if choice.casefold() in row["label"].casefold()
                    ]
            if selected is not None:
                json_write(Path(self.args.result), selected)
        except (KeyboardInterrupt, EOFError):
            pass
        finally:
            Path(self.args.done).touch(mode=0o600)

    def sessions(self):
        result = self.run(
            "list-sessions",
            "-F",
            "#{session_id}\t#{session_name}\t#{session_path}\t#{@workspace-root}\t#{@workspace-internal}",
            check=False,
        )
        entries = []
        for line in result.stdout.splitlines():
            fields = line.split(SEP)
            if len(fields) != 5:
                continue
            sid, name, path, root, internal = fields
            if internal or name.startswith("__workspace-"):
                continue
            entries.append({"id": sid, "name": name, "path": root or path})
        return entries

    def project_rows(self):
        host = socket.gethostname().split(".")[0]
        sessions = self.sessions()
        rows = [
            {
                "label": f"●  {host} / {s['name']}  ·  {s['path']}",
                "kind": "session",
                "value": s["id"],
            }
            for s in sessions
        ]
        known = {str(Path(s["path"]).resolve()) for s in sessions if s["path"]}
        roots = [
            Path.home() / name
            for name in ("dotfiles", "projects", "sndbx", "llunde-new")
        ]
        favorites = config_home() / "favorites"
        candidates = []
        if favorites.is_file():
            candidates += [
                (Path(os.path.expandvars(line.strip())).expanduser(), "★")
                for line in favorites.read_text().splitlines()
                if line.strip() and not line.lstrip().startswith("#")
            ]
        if self.pane:
            candidates.append((self.root(), "◆"))
        for root in roots:
            if root.is_dir():
                candidates.append((root, "◆"))
                if root.name != "dotfiles":
                    with contextlib.suppress(OSError):
                        candidates += [
                            (path, "◆")
                            for path in sorted(root.iterdir())
                            if path.is_dir() and not path.name.startswith(".")
                        ][:300]
        if shutil.which("zoxide"):
            with contextlib.suppress(Error):
                result = call(["zoxide", "query", "--list"], check=False, timeout=2)
                candidates += [
                    (Path(line), "↗") for line in result.stdout.splitlines()[:300]
                ]
        unique = {}
        for path, kind in candidates:
            with contextlib.suppress(OSError):
                if path.is_dir() and "\n" not in str(path) and "\t" not in str(path):
                    unique.setdefault(str(path.resolve()), kind)
        repos = [path for path in unique if (Path(path) / ".git").exists()]

        def worktrees(path):
            if not shutil.which("git"):
                return []
            with contextlib.suppress(Error):
                result = call(
                    ["git", "-C", path, "worktree", "list", "--porcelain", "-z"],
                    check=False,
                    timeout=2,
                )
                return [
                    part.removeprefix("worktree ")
                    for part in result.stdout.split("\0")
                    if part.startswith("worktree ")
                ]
            return []

        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
            for paths in executor.map(worktrees, repos[:100]):
                for path in paths:
                    if "\n" not in path and "\t" not in path:
                        unique.setdefault(path, "⑂")
        for path, kind in unique.items():
            if path not in known:
                rows.append(
                    {
                        "label": f"{kind}  {host} / {Path(path).name}  ·  {path}",
                        "kind": "project",
                        "value": path,
                    }
                )
        return rows

    def project_session(self, path):
        path = str(Path(path).expanduser().resolve())
        if not Path(path).is_dir():
            raise Error("project directory not found")
        sessions = self.sessions()
        for entry in sessions:
            if entry["path"] and str(Path(entry["path"]).resolve()) == path:
                return entry["id"]
        name = session_base(path)
        if any(entry["name"] == name for entry in sessions):
            name += "-" + identifier(path)[:6]
        result = self.run(
            "new-session",
            "-dP",
            "-F",
            "#{session_id}",
            "-s",
            name,
            "-c",
            path,
            "-n",
            "shell",
            check=False,
        )
        if result.returncode:
            # A second picker may have created the same project concurrently.
            for entry in self.sessions():
                if entry["path"] == path:
                    return entry["id"]
            raise Error(clean(result.stderr))
        sid = result.stdout.strip()
        self.run("set-option", "-t", sid, "@workspace-root", path)
        return sid

    def projects(self):
        row = self.choose(
            self.project_rows(),
            "Workspaces",
            header="● running · ★ favorite · ⑂ worktree · ↗ recent",
        )
        if row:
            sid = (
                row["value"]
                if row["kind"] == "session"
                else self.project_session(row["value"])
            )
            if self.client:
                self.run("switch-client", "-c", self.client, "-t", sid)
            else:
                self.attach(sid)

    def bindings(self):
        rows = []
        # Older tmux exposes prefix notes through -N, newer versions add -F.
        for table in ("prefix", "workspace-resize", "copy-mode-vi"):
            result = self.run(
                "list-keys",
                "-F",
                "#{key_string}\t#{key_note}",
                "-T",
                table,
                check=False,
            )
            formatted = result.returncode == 0
            if not formatted:
                result = self.run("list-keys", "-N", "-P", "", "-T", table, check=False)
            for line in result.stdout.splitlines():
                with contextlib.suppress(ValueError, IndexError):
                    key, note = (
                        line.split("\t", 1) if formatted else line.split(None, 1)
                    )
                    if not note.strip():
                        continue
                    label = f"{'P' if table == 'prefix' else table} {key:<9} {note}"
                    rows.append(
                        {"label": label, "kind": "binding", "table": table, "key": key}
                    )
        rows += [
            {
                "label": "agent       Start managed Codex",
                "kind": "action",
                "value": "agent-codex",
            },
            {
                "label": "agent       Start managed Claude",
                "kind": "action",
                "value": "agent-claude",
            },
            {
                "label": "agent       Handoff status",
                "kind": "action",
                "value": "handoff-status",
            },
            {
                "label": "agent       Follow execution to destination",
                "kind": "action",
                "value": "agent-follow",
            },
            {
                "label": "agent       Cancel queued move",
                "kind": "action",
                "value": "handoff-cancel",
            },
            {
                "label": "agent       Recover failed handoff",
                "kind": "action",
                "value": "handoff-recover",
            },
            {"label": "host        Connect archie", "kind": "host", "value": "archie"},
            {"label": "host        Connect macie", "kind": "host", "value": "macie"},
            {
                "label": "keys        Read actual input bytes",
                "kind": "action",
                "value": "inspect-keys",
            },
            {
                "label": "favorites   Favorite this project",
                "kind": "action",
                "value": "favorite",
            },
        ]
        return rows

    def palette(self):
        self.context()
        row = self.choose(
            self.bindings(),
            "Actions",
            header="Bindings come from the running server · P = prefix",
        )
        if not row:
            return
        if row["kind"] == "binding":
            if not self.client:
                raise Error("attached client required")
            if self.server_version < (3, 4):
                return self.legacy_binding(row)
            if row["table"] == "copy-mode-vi":
                self.run("copy-mode", "-t", self.pane)
                self.run("send-keys", "-K", "-c", self.client, row["key"])
            else:
                self.run("switch-client", "-c", self.client, "-T", row["table"])
                self.run("send-keys", "-K", "-c", self.client, row["key"])
        elif row["kind"] == "host":
            self.args.target = row["value"]
            self.host()
        else:
            self.dispatch(row["value"])

    def legacy_binding(self, row):
        # tmux 3.3 has no send-keys -K. Resolve the live command and execute it
        # with explicit pane/client context, never as text into the application.
        rendered = self.run("list-keys", "-T", row["table"]).stdout
        body = None
        for line in rendered.splitlines():
            match = re.match(
                r"^bind-key\s+(?:-r\s+)?-T\s+\S+\s+((?:\\.|\S)+)\s+(.*)$", line
            )
            if match:
                with contextlib.suppress(ValueError, IndexError):
                    if shlex.split(match.group(1))[0] == row["key"]:
                        body = match.group(2)
                        break
        if not body:
            raise Error("binding changed; reopen the action palette")
        lexer = shlex.shlex(body, posix=True, punctuation_chars=";{}")
        lexer.whitespace_split = True
        lexer.commenters = ""
        client_targets = {
            "switch-client": "-c",
            "detach-client": "-t",
            "display-panes": "-t",
            "display-message": "-c",
            "command-prompt": "-t",
            "confirm-before": "-t",
            "display-menu": "-c",
            "display-popup": "-c",
            "refresh-client": "-t",
            "suspend-client": "-t",
            "lock-client": "-t",
        }
        pane_commands = {
            "split-window",
            "new-pane",
            "select-pane",
            "resize-pane",
            "swap-pane",
            "select-layout",
            "next-layout",
            "previous-layout",
            "rotate-window",
            "copy-mode",
            "send-keys",
            "send-prefix",
            "paste-buffer",
            "choose-tree",
            "choose-buffer",
            "choose-client",
            "find-window",
            "run-shell",
            "if-shell",
            "set-option",
            "set-window-option",
            "rename-window",
            "select-window",
            "last-pane",
            "kill-pane",
            "kill-window",
        }
        session_commands = {
            "next-window",
            "previous-window",
            "last-window",
            "rename-session",
            "kill-session",
        }
        session = self.fmt("#{session_id}")
        rebuilt = []
        at_start = True
        previous = ""
        for token in lexer:
            if token in {";", "{", "}"}:
                rebuilt.append(token)
                at_start = token != "}"
                continue
            token = token.replace("#{q:client_name}", shlex.quote(self.client)).replace(
                "#{client_name}", self.client
            )
            token = token.replace("#{q:pane_id}", shlex.quote(self.pane)).replace(
                "#{pane_id}", self.pane
            )
            if previous == "-t" and token.startswith(":"):
                token = session + token
            rebuilt.append(shlex.quote(token))
            if at_start and token in client_targets:
                rebuilt += [client_targets[token], shlex.quote(self.client)]
            elif at_start and token in pane_commands:
                rebuilt += ["-t", shlex.quote(self.pane)]
            elif at_start and token in session_commands:
                rebuilt += ["-t", shlex.quote(session)]
            elif at_start and token == "new-window":
                rebuilt += ["-t", shlex.quote(session + ":")]
            elif at_start and token == "break-pane":
                rebuilt += ["-s", shlex.quote(self.pane)]
            previous = token
            at_start = False
        commands = "select-pane -t " + shlex.quote(self.pane) + " ; "
        if row["table"] == "copy-mode-vi":
            commands += "copy-mode -t " + shlex.quote(self.pane) + " ; "
        commands += " ".join(rebuilt)
        self.run("run-shell", "-C", commands)

    def favorite(self):
        path = str(self.root())
        target = config_home() / "favorites"
        target.parent.mkdir(parents=True, exist_ok=True)
        existing = target.read_text().splitlines() if target.exists() else []
        if path not in existing:
            with target.open("a", encoding="utf-8") as stream:
                stream.write(path + "\n")
        self.notice(f"Favorite: {path}")

    def panes(self):
        fmt = "#{pane_id}\t#{session_id}\t#{session_name}\t#{window_id}\t#{pane_current_command}\t#{pane_current_path}\t#{@workspace-tool}\t#{@workspace-project}\t#{pane_floating_flag}"
        fields = (
            "id",
            "session",
            "session_name",
            "window",
            "command",
            "path",
            "tool",
            "project",
            "floating",
        )
        return [
            dict(zip(fields, line.split(SEP)))
            for line in self.run("list-panes", "-a", "-F", fmt).stdout.splitlines()
            if len(line.split(SEP)) == len(fields)
        ]

    def internal_session(self, name, kind, cwd):
        result = self.run("has-session", "-t", "=" + name, check=False)
        if result.returncode:
            self.run(
                "new-session", "-d", "-s", name, "-c", cwd, "-x", "100", "-y", "30"
            )
        sid = self.run(
            "display-message", "-p", "-t", "=" + name + ":", "#{session_id}"
        ).stdout.strip()
        self.run("set-option", "-t", sid, "@workspace-internal", kind)
        self.run("set-option", "-t", sid, "status", "off")
        return sid

    def shelf_park(self):
        self.context()
        if self.fmt("#{@workspace-tool}") == "scratch-view":
            return self.scratch()
        if self.fmt("#{session_name}") == SHELF:
            raise Error("pane is already on the shelf")
        cwd, origin = self.cwd(), self.fmt("#{session_name}:#{window_name}")
        shelf = self.internal_session(SHELF, "shelf", cwd)
        # Preserve the origin workspace when parking its only pane/window.
        if self.fmt("#{window_panes}") == "1":
            self.run(
                "new-window",
                "-d",
                "-t",
                self.fmt("#{session_id}"),
                "-c",
                cwd,
                "-n",
                "shell",
            )
        self.run(
            "set-option", "-p", "-t", self.pane, "@workspace-origin", clean(origin)
        )
        self.run("set-option", "-p", "-t", self.pane, "@workspace-tool", "shelf")
        self.run(
            "break-pane",
            "-d",
            "-s",
            self.pane,
            "-t",
            shelf + ":",
            "-n",
            clean(origin).replace(":", "-"),
        )
        self.notice("Pane parked · P . to retrieve")

    def shelf(self):
        self.context()
        rows = [
            {"label": f"{p['command']} · {p['path']} · {p['id']}", "value": p["id"]}
            for p in self.panes()
            if p["tool"] == "shelf"
        ]
        row = self.choose(
            rows,
            "Pane shelf",
            header="Live processes · Enter retrieves into this window",
        )
        if row:
            if row["value"] == self.pane:
                return
            self.run("join-pane", "-h", "-s", row["value"], "-t", self.pane)
            self.run("set-option", "-pu", "-t", row["value"], "@workspace-tool")

    def scratch(self):
        self.context()
        current = self.fmt("#{@workspace-tool}")
        root = (
            self.fmt("#{@workspace-project}")
            if current == "scratch-view"
            else str(self.root())
        )
        views = [
            p
            for p in self.panes()
            if p["tool"] == "scratch-view" and p["project"] == root
        ]
        window = self.fmt("#{window_id}")
        for view in views:
            if view["window"] == window:
                # This pane contains only our tmux client; backing jobs survive.
                self.run("kill-pane", "-t", view["id"])
                return
        name = self.internal_session(
            "__workspace-scratch-" + identifier(root), "scratch", root
        )
        self.run("set-option", "-t", name, "prefix", "None")
        self.run("set-option", "-t", name, "prefix2", "None")
        sock = self.socket or self.fmt("#{socket_path}")
        command = [
            "env",
            "-u",
            "TMUX",
            "-u",
            "TMUX_PANE",
            "tmux",
            "-S",
            sock,
            "attach-session",
            "-t",
            name,
        ]
        if self.server_version >= (3, 7):
            pane = self.run(
                "new-pane",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                self.pane,
                "-x",
                "82%",
                "-y",
                "72%",
                "-c",
                root,
                *command,
            ).stdout.strip()
        elif self.server_version >= (3, 2):
            self.popup(
                command,
                "Scratch · Esc closes view, shell persists",
                cwd=root,
                close_on_exit=False,
            )
            return
        else:
            pane = self.run(
                "split-window",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                self.pane,
                "-l",
                "65%",
                "-c",
                root,
                *command,
            ).stdout.strip()
        self.run("set-option", "-p", "-t", pane, "@workspace-tool", "scratch-view")
        self.run("set-option", "-p", "-t", pane, "@workspace-project", root)
        self.run("select-pane", "-t", pane, "-T", "scratch · toggle to hide")

    def yazi(self):
        self.context()
        if not shutil.which("yazi"):
            raise Error("yazi: not installed")
        if self.args.cwd_file:
            with tempfile.TemporaryDirectory(prefix="tmux-yazi-") as directory:
                chosen = Path(directory) / "chosen"
                self.popup(
                    [
                        "yazi",
                        "--chooser-file",
                        chosen,
                        "--cwd-file",
                        self.args.cwd_file,
                        str(self.cwd()),
                    ],
                    "Files",
                )
                if chosen.exists():
                    selected = chosen.read_text().rstrip("\n")
                    if "\n" not in selected and Path(selected).is_dir():
                        Path(self.args.cwd_file).write_text(selected)
        elif self.fmt("#{pane_current_command}") == "zsh":
            self.run("send-keys", "-t", self.pane, "-l", "\033[115;9u")
        else:
            self.popup(["yazi", str(self.cwd())], "Files")

    def launch(self, command):
        program = "lazygit" if command == "lazygit" else "agent-hop"
        if not shutil.which(program):
            raise Error(f"{program}: not installed")
        if command == "lazygit":
            self.popup(["lazygit"], "Git", cwd=self.root())
        elif command == "agent":
            self.popup(["agent-hop"], "Agent sessions", cwd=self.root())
        elif command in {"agent-codex", "agent-claude"}:
            agent = command.removeprefix("agent-")
            if not shutil.which("agent-hop"):
                raise Error("agent-hop: not installed")
            self.run(
                "new-window",
                "-t",
                self.fmt("#{session_id}"),
                "-c",
                self.root(),
                "-n",
                agent,
                "agent-hop",
                "run",
                agent,
            )
        elif command == "handoff":
            result = call(
                ["agent-hop", "move", "--pane", self.pane], cwd=self.root(), check=False
            )
            self.report(result.stdout + result.stderr, "Move execution · q closes")
        elif command == "agent-follow":
            state = call(["agent-hop", "status", "--pane", self.pane], cwd=self.root())
            status = json.loads(state.stdout)
            if status.get("phase") not in {
                "moved",
                "commit-uncertain",
                "source-stopped",
            }:
                self.report(state.stdout, "Handoff pending · destination not ready")
                return
            self.run(
                "new-window",
                "-t",
                self.fmt("#{session_id}"),
                "-c",
                self.root(),
                "-n",
                "agent-remote",
                *self.self_command("_agent-follow-client"),
            )
        elif command == "handoff-status":
            result = call(
                ["agent-hop", "status", "--pane", self.pane],
                check=False,
                cwd=self.root(),
            )
            self.report(result.stdout + result.stderr, "Agent handoff")
        elif command == "handoff-cancel":
            result = call(
                ["agent-hop", "cancel", "--pane", self.pane],
                cwd=self.root(),
                check=False,
            )
            self.report(result.stdout + result.stderr, "Cancel queued move")
        elif command == "handoff-recover":
            self.run(
                "new-window",
                "-t",
                self.fmt("#{session_id}"),
                "-c",
                self.root(),
                "-n",
                "agent-recovery",
                *self.self_command("_agent-recover-client"),
            )

    def agent_client(self):
        operation = (
            "recover" if self.args.command == "_agent-recover-client" else "follow"
        )
        result = call(
            ["agent-hop", operation, "--pane", self.pane], capture=False, check=False
        )
        if result.returncode:
            print(f"Agent {operation} exited {result.returncode}.")
            with contextlib.suppress(EOFError):
                input("Enter to close")
        return result.returncode

    def plugin(self, operation):
        command = [str(HERE / "tmux-plugins"), operation]
        if self.pane:
            command += ["--pane", self.pane]
        if self.client:
            command += ["--client", self.client]
        env = dict(os.environ)
        if self.socket:
            env["TMUX_WORKSPACE_SOCKET"] = self.socket
        return call(command, check=False, env=env)

    def quick_select(self):
        self.context()
        result = self.plugin("fingers")
        if result.returncode == 0:
            return
        text = self.run("capture-pane", "-pJ", "-t", self.pane).stdout
        pattern = r"https?://[^\s<>\"']+|(?:~|\.{1,2})?/[^\s<>\"']+|\b[0-9a-f]{7,40}\b"
        values = list(dict.fromkeys(re.findall(pattern, text)))
        row = self.choose(
            [{"label": value, "value": value} for value in values],
            "Quick select",
            header="Paths · URLs · hashes · Enter copies",
        )
        if row:
            self.copy(row["value"])

    def copy(self, value):
        cmd = ["set-buffer", "-w"]
        if self.client:
            cmd += ["-t", self.client]
        result = self.run(*cmd, "--", value, check=False)
        if result.returncode:
            self.run("load-buffer", "-", input=value)
        self.notice("Copied")

    def output(self):
        self.context()
        already_copying = self.fmt("#{pane_in_mode}") != "0"
        self.run("copy-mode", "-t", self.pane)
        captured = self.run(
            "capture-pane", "-p", "-t", self.pane, "-S", "-100000"
        ).stdout.splitlines()
        rows = [
            {"label": f"{index + 1:>6}  {clean(line, 1200)}", "value": index + 1}
            for index, line in enumerate(captured)
            if line.strip()
        ]
        row = self.choose(
            rows,
            "Scrollback",
            header="Search 100,000 lines · Enter jumps to the selected line",
        )
        if row:
            self.run("send-keys", "-X", "-t", self.pane, "history-top")
            if row["value"] > 1:
                self.run(
                    "send-keys",
                    "-X",
                    "-N",
                    str(row["value"] - 1),
                    "-t",
                    self.pane,
                    "cursor-down",
                )
        elif not already_copying:
            self.run("send-keys", "-X", "-t", self.pane, "cancel")

    def recover(self, operation):
        if operation == "restore":
            row = self.choose(
                [
                    {"label": "Cancel", "value": False},
                    {
                        "label": "Restore server-wide layouts and supported programs · existing panes preserved",
                        "value": True,
                    },
                ],
                "Workspace recovery",
                header="Saved process memory, network connections and active turns are not restored",
            )
            if not row or not row["value"]:
                return
        result = self.plugin(operation)
        if result.returncode:
            raise Error(
                result.stderr.strip() or result.stdout.strip() or f"{operation} failed"
            )
        self.notice("Workspace saved" if operation == "save" else "Workspace restored")

    def close(self, window=False):
        self.context()
        command = (
            ["kill-window", "-t", self.fmt("#{window_id}")]
            if window
            else ["kill-pane", "-t", self.pane]
        )
        # Always confirm. A shell's foreground name does not reveal its background jobs.
        what = "window and its processes" if window else "pane and its processes"
        args = ["confirm-before", "-p", f"Close {what}? (y/n)"]
        if self.client:
            args += ["-t", self.client]
        self.run(*args, shlex.join(command))

    def report(self, value, title):
        with tempfile.TemporaryDirectory(prefix="tmux-report-") as directory:
            path = Path(directory) / "report"
            path.write_text(value, encoding="utf-8")
            path.chmod(0o600)
            if self.client:
                self.popup(self.self_command("_report", "--data", path), title)
            else:
                print(value)

    def inspect(self):
        self.context()
        fields = [
            "host",
            "version",
            "client_name",
            "client_pid",
            "client_created",
            "client_termname",
            "client_termfeatures",
            "client_key_table",
            "client_prefix",
            "pane_id",
            "pane_current_command",
            "pane_key_mode",
            "pane_in_mode",
            "pane_floating_flag",
            "@workspace-client-label",
        ]
        lines = [
            f"{field:26} {self.fmt('#{' + ('E:' if field == '@workspace-client-label' else '') + field + '}')}"
            for field in fields
        ]
        for option in (
            "extended-keys",
            "extended-keys-format",
            "escape-time",
            "set-clipboard",
        ):
            lines.append(
                f"{option:26} {self.run('show-options', '-sv', option, check=False).stdout.strip()}"
            )
        lines += [
            "",
            "Desktop gesture → WezTerm adapter → tmux key table → application",
            "P P forwards a prefix into a nested remote tmux.",
            "P Space → Read actual input bytes: checks bytes reaching a tmux pane.",
            "The terminal may consume a key before tmux sees it.",
            "",
            "Registered actions:",
        ]
        lines += [row["label"] for row in self.bindings()]
        self.report("\n".join(lines), "Key routing")

    def inspect_keys(self):
        self.context()
        self.run(
            "split-window", "-t", self.pane, "-l", "40%", *self.self_command("_keys")
        )

    def key_reader(self):
        print(
            "Press keys. Bytes shown after tmux decoding; Ctrl-C exits.\r\n", flush=True
        )
        fd = sys.stdin.fileno()
        previous = termios.tcgetattr(fd)
        try:
            tty.setraw(fd)
            while True:
                data = os.read(fd, 128)
                if data == b"\x03" or not data:
                    break
                # CSI sequences may arrive in multiple reads.
                while select.select([fd], [], [], 0.03)[0]:
                    data += os.read(fd, 128)
                os.write(
                    sys.stdout.fileno(),
                    ("  " + data.hex(" ") + "   " + repr(data) + "\r\n").encode(),
                )
        finally:
            termios.tcsetattr(fd, termios.TCSADRAIN, previous)

    def client_rows(self):
        fields = ("name", "pid", "created", "tty", "term", "internal", "session")
        fmt = "#{client_name}\t#{client_pid}\t#{client_created}\t#{client_tty}\t#{client_termname}\t#{@workspace-internal}\t#{client_session}"
        return [
            dict(zip(fields, line.split(SEP)))
            for line in self.run(
                "list-clients", "-F", fmt, check=False
            ).stdout.splitlines()
            if len(line.split(SEP)) == len(fields)
        ]

    def client_update(self, remove=False):
        sock = self.socket or self.fmt("#{socket_path}", check=False)
        if not sock:
            return
        lock = Path(sock).parent / (Path(sock).name + ".workspace-clients.lock")
        descriptor = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            self._client_update(remove)
        finally:
            os.close(descriptor)

    def _client_update(self, remove=False):
        rows = self.client_rows()
        was_internal = any(
            row["name"] == self.client and row["internal"] for row in rows
        )
        if remove:
            rows = [row for row in rows if row["name"] != self.client]
        marker_tty = (
            None
            if was_internal
            else (self.args.tty or (self.client if remove else None))
        )
        for row in rows:
            if row["name"] == self.client and not row["internal"]:
                marker_tty = row["tty"]
        if marker_tty:
            self.write_marker(marker_tty, not remove)
        label_format = "#{client_termname}"
        for row in reversed(rows):
            key = "@workspace-client-" + row["pid"] + "-" + row["created"]
            stored = self.run("show-options", "-gqv", key, check=False).stdout.strip()
            if self.args.from_host and row["name"] == self.client:
                stored = (
                    clean(self.args.from_host, 60)
                    + " → "
                    + socket.gethostname().split(".")[0]
                )
                stored = re.sub(r"[^\w .@:/→+-]", "", stored)
                self.run("set-option", "-g", key, stored)
            if not stored:
                stored = clean(row["term"], 50)
            condition = (
                "#{&&:#{==:#{client_pid},"
                + row["pid"]
                + "},#{==:#{client_created},"
                + row["created"]
                + "}}"
            )
            label_format = (
                "#{?"
                + condition
                + ","
                + stored.replace("#", "##").replace(",", "")
                + ","
                + label_format
                + "}"
            )
        self.run(
            "set-option", "-g", "@workspace-client-label", label_format, check=False
        )
        live_keys = {
            "@workspace-client-" + row["pid"] + "-" + row["created"] for row in rows
        }
        for line in self.run("show-options", "-g", check=False).stdout.splitlines():
            key = line.partition(" ")[0]
            if re.fullmatch(r"@workspace-client-\d+-\d+", key) and key not in live_keys:
                self.run("set-option", "-gu", key, check=False)

    def write_marker(self, target, enabled):
        if not target.startswith("/dev/"):
            return
        try:
            details = os.stat(target)
            if not stat.S_ISCHR(details.st_mode) or details.st_uid != os.getuid():
                return
            fd = os.open(target, os.O_WRONLY | os.O_NONBLOCK | os.O_NOCTTY)
            try:
                if os.isatty(fd):
                    terminal_marker(fd, enabled)
            finally:
                os.close(fd)
        except OSError:
            pass

    def attach(self, sid):
        env = dict(os.environ)
        env.pop("TMUX", None)
        env.pop("TMUX_PANE", None)
        proc = subprocess.Popen(self.tmux + ["attach-session", "-t", sid], env=env)
        registered = False
        try:
            if sys.stdout.isatty():
                terminal_marker(sys.stdout.fileno(), True)
            deadline = time.monotonic() + 3
            while proc.poll() is None and time.monotonic() < deadline:
                for row in self.client_rows():
                    if row["pid"] == str(proc.pid):
                        self.client = row["name"]
                        self.client_update()
                        registered = True
                        break
                if registered:
                    break
                time.sleep(0.05)
            return proc.wait()
        finally:
            if sys.stdout.isatty():
                terminal_marker(sys.stdout.fileno(), False)
            if registered:
                self.client_update(remove=True)

    def enter(self):
        target = self.args.target
        if target and Path(target).expanduser().is_dir():
            sid = self.project_session(target)
        elif self.args.session or target:
            name = self.args.session or target
            if (
                name.startswith("-")
                or not clean(name) == name
                or ":" in name
                or "." in name
            ):
                raise Error(
                    "session name: use letters, digits, spaces, underscores or hyphens"
                )
            existing = next((s for s in self.sessions() if s["name"] == name), None)
            if existing:
                sid = existing["id"]
            else:
                sid = self.run(
                    "new-session",
                    "-dP",
                    "-F",
                    "#{session_id}",
                    "-s",
                    name,
                    "-c",
                    Path.cwd(),
                ).stdout.strip()
        else:
            sid = self.project_session(Path.cwd())
        if os.environ.get("TMUX") and not self.args.from_host:
            self.context()
            if not self.client:
                raise Error("multiple clients attached; use P s or pass --client")
            self.run("switch-client", "-c", self.client, "-t", sid)
        else:
            return self.attach(sid)

    def host(self):
        target = self.args.target
        if not target:
            row = self.choose(
                [
                    {"label": name, "value": name}
                    for name in ("archie", "macie")
                    if name != socket.gethostname().split(".")[0]
                ],
                "Connect host",
            )
            if not row:
                return
            target = row["value"]
        if not re.fullmatch(r"[a-zA-Z0-9_][a-zA-Z0-9_.@-]*", target):
            raise Error("invalid SSH host")
        if target == socket.gethostname().split(".")[0]:
            self.args.target = None
            return self.projects() if self.pane else self.enter()
        origin = socket.gethostname().split(".")[0]
        # SSH resolves the user's route policy; no transport is guessed from OS.
        remote = 'unset HWIRE_SESSION TMUX TMUX_PANE; export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"; '
        remote += (
            'exec "$HOME/.config/tmux/bin/tmux-workspace" enter --from '
            + shlex.quote(origin)
        )
        remote += " --session " + shlex.quote(self.args.session or "main")
        argv = [
            "ssh",
            "-tt",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "LogLevel=ERROR",
            "--",
            target,
            remote,
        ]
        if self.args.command != "_host-client" and (
            self.pane or os.environ.get("TMUX")
        ):
            self.context()
            launcher = [
                str(EXECUTABLE),
                "_host-client",
                target,
                "--session",
                self.args.session or "main",
            ]
            self.run(
                "new-window", "-t", self.fmt("#{session_id}"), "-n", target, *launcher
            )
        else:
            status = call(argv, capture=False, check=False).returncode
            if status and self.args.command == "_host-client":
                print(f"Connection to {target} exited {status}.")
                with contextlib.suppress(EOFError):
                    input("Enter to close")
            return status

    def reload(self):
        configured = self.run(
            "show-options", "-gqv", "@workspace_config", check=False
        ).stdout.strip()
        paths = ([Path(configured) / ".tmux.conf"] if configured else []) + [
            config_home() / "tmux.conf",
            Path.home() / ".tmux.conf",
        ]
        source = next((path for path in paths if path.is_file()), None)
        if source is None:
            raise Error("tmux.conf not found")
        module_root = Path(configured) if configured else source.resolve().parent
        with tempfile.TemporaryDirectory(prefix="tmux-validate-") as directory:
            sock = str(Path(directory) / "socket")
            command = ["tmux", "-S", sock]
            environment = dict(os.environ, DOTFILES_TMUX_VALIDATE="1")
            environment.pop("TMUX", None)
            environment.pop("TMUX_PANE", None)
            try:
                call(
                    command
                    + [
                        "-f",
                        "/dev/null",
                        "new-session",
                        "-d",
                        "-s",
                        "validate",
                        "/bin/sh",
                    ],
                    env=environment,
                )
                call(
                    command
                    + ["set-option", "-g", "@workspace_config", str(module_root)],
                    env=environment,
                )
                checked = call(
                    command + ["source-file", str(source)], env=environment, check=False
                )
                errors = (checked.stdout + checked.stderr).strip()
                if checked.returncode or errors:
                    raise Error(
                        "reload validation: "
                        + clean(errors or "configuration failed", 1500)
                    )
            finally:
                call(command + ["kill-server"], env=environment, check=False)
        result = self.run("source-file", source, check=False)
        if result.returncode or result.stderr.strip():
            raise Error("reload: " + clean(result.stderr or result.stdout, 1000))
        original_client = self.client
        try:
            for row in self.client_rows():
                if not row["internal"]:
                    self.client = row["name"]
                    self.client_update()
        finally:
            self.client = original_client
        self.notice("Tmux reloaded")

    def doctor(self):
        lines = [
            f"host        {socket.gethostname()}",
            f"tmux        {self.run('-V').stdout.strip()}",
            f"controller  {EXECUTABLE}",
        ]
        for tool in (
            "fzf",
            "git",
            "zoxide",
            "lazygit",
            "yazi",
            "agent-hop",
            "ssh",
            "python3",
        ):
            lines.append(f"{tool:12}{shutil.which(tool) or 'missing'}")
        if self.pane:
            self.context()
            lines += [
                f"socket      {self.fmt('#{socket_path}')}",
                f"scratch     {'native non-modal float' if self.server_version >= (3, 7) else 'popup' if self.server_version >= (3, 2) else 'split pane'}",
            ]
        if (HERE / "tmux-plugins").exists():
            result = self.plugin("status")
            lines += ["", result.stdout, result.stderr]
        self.report("\n".join(lines), "Workspace doctor")

    def dispatch(self, command=None):
        command = command or self.args.command
        if command in {
            "lazygit",
            "agent",
            "agent-codex",
            "agent-claude",
            "handoff",
            "handoff-status",
            "agent-follow",
            "handoff-cancel",
            "handoff-recover",
        }:
            self.context()
            return self.launch(command)
        if command in {"save", "restore"}:
            return self.recover(command)
        if command in {"close-pane", "close-window"}:
            return self.close(window=command == "close-window")
        if command == "client-remove":
            return self.client_update(remove=True)
        if command == "_report":
            path = Path(self.args.data)
            if shutil.which("less"):
                return call(
                    ["less", "-R", "--", path], capture=False, check=False
                ).returncode
            print(path.read_text())
            with contextlib.suppress(EOFError):
                input("Enter to close")
            return
        aliases = {
            "help": "palette",
            "_pick": "pick",
            "_keys": "key_reader",
            "_host-client": "host",
            "_agent-follow-client": "agent_client",
            "_agent-recover-client": "agent_client",
        }
        return getattr(self, aliases.get(command, command.replace("-", "_")))()


def parser():
    parser = argparse.ArgumentParser(
        description="Projects, tools and persistent tmux workspaces"
    )
    parser.add_argument(
        "command",
        choices=[
            "enter",
            "host",
            "projects",
            "palette",
            "help",
            "favorite",
            "shelf-park",
            "shelf",
            "scratch",
            "lazygit",
            "yazi",
            "agent",
            "agent-codex",
            "agent-claude",
            "handoff",
            "handoff-status",
            "agent-follow",
            "handoff-cancel",
            "handoff-recover",
            "inspect",
            "inspect-keys",
            "output",
            "quick-select",
            "save",
            "restore",
            "doctor",
            "reload",
            "close-pane",
            "close-window",
            "client-update",
            "client-remove",
            "_pick",
            "_report",
            "_keys",
            "_host-client",
            "_agent-follow-client",
            "_agent-recover-client",
        ],
    )
    parser.add_argument("target", nargs="?")
    parser.add_argument("--pane")
    parser.add_argument("--client")
    parser.add_argument("--socket")
    parser.add_argument("--session")
    parser.add_argument("--from", dest="from_host")
    parser.add_argument("--tty")
    parser.add_argument("--cwd-file")
    parser.add_argument("--data")
    parser.add_argument("--result")
    parser.add_argument("--done")
    return parser


def main(argv=None):
    args = parser().parse_args(argv)
    workspace = Workspace(args)
    try:
        return workspace.dispatch() or 0
    except (Error, OSError, ValueError) as exc:
        message = "tmux-workspace: " + str(exc)
        print(message, file=sys.stderr)
        if args.command not in {"client-update", "client-remove"}:
            with contextlib.suppress(Error):
                workspace.notice(message)
        return 1
    except KeyboardInterrupt:
        return 130
