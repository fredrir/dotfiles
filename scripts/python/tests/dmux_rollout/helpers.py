import json
import subprocess
from pathlib import Path

from tools.dmux_rollout import service_env
from tools.dmux_rollout.command import Result, Runner
from tools.dmux_rollout.model import Release
from tools.dmux_rollout.workflow import MAC_ENV_LOADER_LABEL, Workflow, WorkflowConfig


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def pushed_repo(root: Path, name: str) -> Path:
    remote = root / f"{name}.git"
    repo = root / name
    subprocess.run(["git", "init", "--bare", str(remote)], check=True, capture_output=True)
    subprocess.run(["git", "init", "-b", "main", str(repo)], check=True, capture_output=True)
    git(repo, "config", "user.name", "Rollout Test")
    git(repo, "config", "user.email", "rollout@example.invalid")
    (repo / "source.txt").write_text("one\n", encoding="utf-8")
    git(repo, "add", "source.txt")
    git(repo, "commit", "-m", "initial")
    git(repo, "remote", "add", "origin", str(remote))
    git(repo, "push", "-u", "origin", "main")
    return repo


def source(repo: Path, commit: str) -> dict:
    return {
        "repo": str(repo),
        "commit": commit,
        "origin": str(repo),
        "remote_refs": ["origin/main"],
        "main_worktree_dirty": [],
    }


def release(tmp_path: Path) -> Release:
    commit = "1" * 40
    return Release.create(
        release_id="20260817-test",
        dotfiles=source(tmp_path / "dotfiles", commit),
        wezterm=source(tmp_path / "wezterm", "2" * 40),
        smoke_name="rollout-smoke",
        archie_host="archie",
    )


def config(tmp_path: Path, dotfiles: Path, wezterm: Path, **overrides) -> WorkflowConfig:
    return WorkflowConfig(
        **{
            "dotfiles_repo": dotfiles,
            "wezterm_repo": wezterm,
            "packages_root": tmp_path / "packages",
            "mac_app": tmp_path / "WezTerm.app",
            "mac_dmux": tmp_path / "bin/dmux",
            "mac_pane_bootstrap": tmp_path / "bin/pane-bootstrap",
            "mac_service_env": tmp_path / "home/.config/dmux/service.env",
            "mac_env_loader_plist": tmp_path / "home/Library/LaunchAgents/loader.plist",
            **overrides,
        }
    )


DURABLE_WEZ_FIRST = (
    "process=1 launchd=1 file=1; durable Wez-first: ~/.config/dmux/service.env is loaded "
    "into launchd"
)
RUNTIME_ONLY = (
    "process=1 launchd=1 file=unset; runtime-only Wez-first: launchd carries 1 but "
    "~/.config/dmux/service.env does not, so a reboot clears it"
)
NO_PREFERENCE = "process=unset launchd=unset file=unset; no preference stated anywhere, the tracked default applies"


class LaunchdFake(Runner):
    """launchctl with a session environment; the loader applies the file."""

    def __init__(self, service_env_path: Path):
        self.path = service_env_path
        self.session: dict[str, str] = {}
        self.sent: list[list[str]] = []

    def capture(self, argv, **kwargs):
        argv = list(argv)
        self.sent.append(argv)
        if argv[:2] == ["launchctl", "kickstart"] and argv[-1].endswith(MAC_ENV_LOADER_LABEL):
            if self.path.exists():
                self.session.update(service_env.parse(self.path.read_text(), name="fake"))
        elif argv[:2] == ["launchctl", "getenv"]:
            return Result(tuple(argv), 0, self.session.get(argv[2], "") + "\n", "")
        elif argv[:2] == ["launchctl", "unsetenv"]:
            self.session.pop(argv[2], None)
        return Result(tuple(argv), 0, "", "")


class MacFake(LaunchdFake):
    """A ready Mac owner: descriptor, native inventory, recovery and doctor.

    Everything the Mac owner snapshot reads is answered from attributes, so a
    test can restart the mux (`restart()`), add user Spaces (`rows`), or change
    what doctor says (`doctor_ok`, `doctor_detail`, `doctor_extra`).
    """

    EPOCH = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    INSTANCE = "6ef8d4c9-0000-4000-8000-000000000000"

    def __init__(self, tmp_path: Path, cfg: WorkflowConfig):
        super().__init__(cfg.mac_service_env)
        self.cfg = cfg
        self.runtime = tmp_path / "runtime" / "dmux"
        self.runtime.mkdir(parents=True)
        self.pid = 5458
        self.epoch = self.EPOCH
        self.rows = [
            {"window_id": 0, "tab_id": 0, "pane_id": 0, "workspace": f"dmux:system:{self.epoch}"}
        ]
        self.clients: list = []
        self.doctor_ok = True
        self.doctor_detail = DURABLE_WEZ_FIRST
        self.doctor_extra: dict = {}
        self.doctor_calls = 0
        self.responders: list = []
        self.write_descriptor()

    def write_descriptor(self) -> None:
        descriptor = {
            "state": "ready",
            "epoch": self.epoch,
            "pid": self.pid,
            "socket": str(self.runtime / "wez-dmux.sock"),
            "socket_dev": 42,
            "socket_ino": 84,
            "start_token": f"macos:{self.pid}",
            "backend_instance_uid": self.INSTANCE,
            "sentinel_window_id": 0,
            "sentinel_tab_id": 0,
            "sentinel_pane_id": 0,
        }
        (self.runtime / "wez-dmux.json").write_text(json.dumps(descriptor))

    def restart(self) -> None:
        """A new incarnation: new pid and epoch, same backend instance."""
        self.pid += 1
        self.epoch = self.epoch[:-4] + f"{self.pid:04d}"
        self.rows[0]["workspace"] = f"dmux:system:{self.epoch}"
        self.write_descriptor()

    def doctor_document(self) -> dict:
        return {
            "schema_version": 1,
            "ok": True,
            "action": "doctor",
            "result": {
                "host": {"ok": True, "detail": "macie (macos)"},
                "wez_first": {"ok": self.doctor_ok, "detail": self.doctor_detail},
                **self.doctor_extra,
            },
            "errors": [],
            "authority_revision": 3,
        }

    def capture(self, argv, **kwargs):
        argv = list(argv)
        for matches, respond in self.responders:
            if matches(argv):
                self.sent.append(argv)
                return respond(argv, kwargs)
        dmux = str(self.cfg.mac_dmux)
        if argv[0] == "getconf":
            self.sent.append(argv)
            return Result(tuple(argv), 0, f"{self.runtime.parent}/\n", "")
        if argv[0] == "ps":
            self.sent.append(argv)
            return Result(
                tuple(argv), 0, f"{self.pid} Sun Aug 23 10:00:00 2026 wezterm-mux-server\n", ""
            )
        if "cli" in argv and "list-clients" in argv:
            self.sent.append(argv)
            return Result(tuple(argv), 0, json.dumps(self.clients), "")
        if "cli" in argv and "list" in argv:
            self.sent.append(argv)
            return Result(tuple(argv), 0, json.dumps(self.rows), "")
        if argv[0] == dmux and argv[1:3] == ["recovery", "status"]:
            self.sent.append(argv)
            status = {
                "state": "ready",
                "server_epoch": self.epoch,
                "backend_instance_uid": self.INSTANCE,
                "generation_uid": "gen",
                "manifest_id": "man",
            }
            return Result(
                tuple(argv), 0, json.dumps({"ok": True, "result": {"status": status}}), ""
            )
        if argv[0] == dmux and argv[1] == "doctor":
            self.sent.append(argv)
            self.doctor_calls += 1
            return Result(tuple(argv), 0, json.dumps(self.doctor_document()), "")
        return super().capture(argv, **kwargs)


def mac_workflow(tmp_path: Path, **overrides):
    cfg = config(tmp_path, tmp_path / "dotfiles", tmp_path / "wezterm", **overrides)
    fake = MacFake(tmp_path, cfg)
    from tools.dmux_rollout.storage import RolloutStore

    store = RolloutStore(tmp_path / "state")
    return Workflow(store, fake, cfg), fake, store
