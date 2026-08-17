from __future__ import annotations

import json
import os
import re
import shutil
import signal
import stat
import time
import uuid
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from tools.dmux_rollout.command import Runner, remote_argv, sha256_file
from tools.dmux_rollout.errors import Refusal, RolloutError, StateError
from tools.dmux_rollout.model import Release, require_commit, require_space_uid
from tools.dmux_rollout.storage import RolloutStore

WORKSPACE_RE = re.compile(r"^dmux:(?P<host>[0-9a-f-]{36}):(?P<space>[0-9a-f-]{36})$")
SYSTEM_WORKSPACE_RE = re.compile(r"^dmux:system:(?P<epoch>[0-9a-f-]{36})$")
RECEIPT_RE = re.compile(
    r"^dmux://(?P<host>[0-9a-f-]{36})/spaces/(?P<space>[0-9a-f-]{36})"
    r"\tbackend=(?P<backend>wez|tmux)\tcreated=(?:true|false)"
)
PACKAGE_RE = re.compile(
    r"^wezterm-fredrir-git(?P<debug>-debug)?-(?P<version>[^/]+)-1-x86_64\.pkg\.tar\.zst$"
)
AMBIENT_MUX_VARS = (
    "TMUX",
    "TMUX_PANE",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "DMUX_SPACE_UID",
    "DMUX_SPACE_NO",
    "DMUX_GROUP_REF",
    "DMUX_SPLIT_REF",
    "DMUX_BACKEND",
    "DMUX_HOST_UID",
    "DMUX_SERVER_EPOCH",
    "DMUX_BACKEND_INSTANCE_UID",
    "DMUX_TMUX_CLIENT_UID",
)


def scrubbed_env(*assignments: str) -> list[str]:
    argv = ["env"]
    for name in AMBIENT_MUX_VARS:
        argv.extend(("-u", name))
    argv.extend(assignments)
    return argv


@dataclass(frozen=True)
class WorkflowConfig:
    dotfiles_repo: Path
    wezterm_repo: Path
    packages_root: Path
    archie_host: str = "archie"
    archie_home: Path = Path("/home/fredrir")
    archie_dotfiles_repo: Path = Path("/home/fredrir/dotfiles")
    archie_wezterm_repo: Path = Path("/home/fredrir/packages/wezterm-dmux-build-w6")
    mac_app: Path = Path("/Applications/WezTerm.app")
    mac_dmux: Path = Path.home() / ".local" / "bin" / "dmux"
    mac_pane_bootstrap: Path = Path.home() / ".local" / "bin" / "pane-bootstrap"

    @classmethod
    def production(cls, dotfiles_repo: Path) -> WorkflowConfig:
        home = Path.home()
        wezterm = Path(os.environ.get("DMUX_WEZTERM_SOURCE", home / "packages/wezterm-dmux"))
        packages = Path(
            os.environ.get("DMUX_ROLLOUT_ARTIFACT_ROOT", home / "packages/dmux-rollouts")
        )
        if not wezterm.is_absolute() or not packages.is_absolute():
            raise StateError("WezTerm source and rollout artifact roots must be absolute")
        return cls(
            dotfiles_repo=dotfiles_repo.absolute(),
            wezterm_repo=wezterm.absolute(),
            packages_root=packages.absolute(),
        )


class Workflow:
    def __init__(self, store: RolloutStore, runner: Runner, config: WorkflowConfig):
        self.store = store
        self.runner = runner
        self.config = config

    # Planning -------------------------------------------------------------

    def plan(
        self,
        *,
        dotfiles_ref: str = "HEAD",
        wezterm_ref: str = "HEAD",
        release_id: str | None = None,
        smoke_name: str,
        smoke_space_uid: str | None = None,
        smoke_host_uid: str | None = None,
    ) -> Release:
        dotfiles = self._git_source(self.config.dotfiles_repo, dotfiles_ref)
        wezterm = self._git_source(self.config.wezterm_repo, wezterm_ref)
        chosen = release_id or self._default_release_id(dotfiles["commit"], wezterm["commit"])
        if self.store.exists(chosen):
            existing = self.store.load(chosen)
            for source, current in (("dotfiles", dotfiles), ("wezterm", wezterm)):
                frozen = existing.data["frozen"][source]
                for field in ("repo", "commit", "origin"):
                    if frozen[field] != current[field]:
                        raise Refusal(f"release {chosen} already freezes different source facts")
            if existing.data["smoke"]["name"] != smoke_name:
                raise Refusal(f"release {chosen} already owns another smoke Space name")
            return existing
        release = Release.create(
            release_id=chosen,
            dotfiles=dotfiles,
            wezterm=wezterm,
            smoke_name=smoke_name,
            archie_host=self.config.archie_host,
        )
        if (smoke_space_uid is None) != (smoke_host_uid is None):
            raise StateError("smoke SpaceUid and HostUid must be supplied together")
        if smoke_space_uid is not None and smoke_host_uid is not None:
            release.set_smoke_identity(space_uid=smoke_space_uid, host_uid=smoke_host_uid)
        self.store.create(release)
        return release

    def _git_source(self, repo: Path, ref: str) -> dict[str, Any]:
        if not repo.is_absolute() or not (repo / ".git").exists():
            raise StateError(f"not a Git checkout: {repo}")
        self.runner.capture(["git", "-C", str(repo), "fetch", "--prune", "origin"])
        commit = self.runner.capture(
            ["git", "-C", str(repo), "rev-parse", "--verify", f"{ref}^{{commit}}"]
        ).stdout.strip()
        require_commit(commit, f"{repo.name} commit")
        remote_refs = self.runner.capture(
            ["git", "-C", str(repo), "branch", "-r", "--contains", commit],
            check=False,
        ).stdout.splitlines()
        remote_refs = [line.strip() for line in remote_refs if "->" not in line and line.strip()]
        if not remote_refs:
            raise Refusal(f"{repo} commit {commit} is not contained in a remote branch")
        origin = self.runner.capture(
            ["git", "-C", str(repo), "remote", "get-url", "origin"]
        ).stdout.strip()
        dirty_raw = self.runner.capture(
            ["git", "-C", str(repo), "status", "--porcelain=v1", "-z"]
        ).stdout
        dirty = sorted(entry for entry in dirty_raw.split("\0") if entry)
        return {
            "repo": str(repo),
            "commit": commit,
            "origin": origin,
            "remote_refs": remote_refs,
            "main_worktree_dirty": dirty,
        }

    @staticmethod
    def _default_release_id(dotfiles: str, wezterm: str) -> str:
        day = datetime.now(UTC).strftime("%Y%m%d")
        return f"{day}-{dotfiles[:8]}-{wezterm[:8]}"

    # Artifact worktrees/builds -------------------------------------------

    def build(self, release: Release, *, skip_tests: bool = False) -> Release:
        root = self._artifact_root(release)
        self._require_artifact_dir(root, create=True)
        dotfiles = self._ensure_worktree(release, "dotfiles", root / "worktrees/dotfiles-mac")
        wezterm = self._ensure_worktree(release, "wezterm", root / "worktrees/wezterm-mac")
        self._require_clean_frozen_worktree(release, "dotfiles", dotfiles)
        self._require_clean_frozen_worktree(release, "wezterm", wezterm)

        # The dmux integration suite exercises fork-only protocol surfaces
        # against a live scratch mux.  Build and freeze the exact fork first;
        # the test environment below then points at those release artifacts
        # instead of an installed or historical developer build.
        self._build_mac_wezterm(release, root, dotfiles, wezterm, skip_tests=skip_tests)
        self._build_mac_dotfiles(release, root, dotfiles, skip_tests=skip_tests)
        release.set_phase("built")
        self.store.save(release)
        return release

    def _build_mac_wezterm(
        self,
        release: Release,
        root: Path,
        dotfiles: Path,
        wezterm: Path,
        *,
        skip_tests: bool,
    ) -> None:
        if release.completed("build.mac.wezterm"):
            self._verify_artifact_set(release.data["artifacts"].get("mac_wezterm"), "mac_wezterm")
            return

        target = root / "targets/wezterm-mac"
        log = self._log_path(release, "build-mac-wezterm.log")
        build_env = {"CARGO_TARGET_DIR": str(target)}
        if not skip_tests:
            for command in (
                ["cargo", "test", "-p", "codec", "--", "--test-threads=1"],
                ["cargo", "test", "-p", "mux", "--", "--test-threads=1"],
                [
                    "cargo",
                    "test",
                    "-p",
                    "wezterm-gui",
                    "dmux",
                    "--",
                    "--test-threads=1",
                ],
            ):
                self.runner.stream(
                    command,
                    cwd=wezterm,
                    env=build_env,
                    unset_env=AMBIENT_MUX_VARS,
                    log=log,
                )
            self.runner.stream(
                ["sh", str(dotfiles / "shared/wezterm/wez/dmux_bridge/tests/fork_surface.sh")],
                cwd=dotfiles,
                env={**build_env, "DMUX_WEZTERM_SOURCE": str(wezterm)},
                unset_env=AMBIENT_MUX_VARS,
                log=log,
            )
        self.runner.stream(
            [
                "cargo",
                "build",
                "--release",
                "-p",
                "wezterm",
                "-p",
                "wezterm-gui",
                "-p",
                "wezterm-mux-server",
            ],
            cwd=wezterm,
            env=build_env,
            unset_env=AMBIENT_MUX_VARS,
            log=log,
        )
        artifacts = self._copy_artifacts(
            root / "artifacts/macos/wezterm",
            {
                "wezterm": target / "release/wezterm",
                "wezterm-gui": target / "release/wezterm-gui",
                "wezterm-mux-server": target / "release/wezterm-mux-server",
            },
        )
        version = self.runner.capture([str(target / "release/wezterm"), "--version"]).stdout.strip()
        if release.data["frozen"]["wezterm"]["commit"][:8] not in version:
            raise Refusal(f"built WezTerm version does not contain frozen commit: {version}")
        release.data["artifacts"]["mac_wezterm"] = artifacts
        release.data["artifacts"]["mac_wezterm_version"] = version
        self.store.checkpoint(
            release,
            "build.mac.wezterm",
            {"artifacts": artifacts, "version": version},
        )

    def _build_mac_dotfiles(
        self,
        release: Release,
        root: Path,
        dotfiles: Path,
        *,
        skip_tests: bool,
    ) -> None:
        if release.completed("build.mac.dotfiles"):
            self._verify_artifact_set(release.data["artifacts"].get("mac_dotfiles"), "mac_dotfiles")
            return

        target = root / "targets/dotfiles-mac"
        log = self._log_path(release, "build-mac-dotfiles.log")
        test_env = self._mac_dmux_test_environment(release, target)
        if not skip_tests:
            self.runner.stream(
                ["cargo", "test", "-p", "dmux", "--", "--test-threads=1"],
                cwd=dotfiles / "scripts/rust",
                env=test_env,
                unset_env=AMBIENT_MUX_VARS,
                log=log,
            )
        self.runner.stream(
            [
                "cargo",
                "build",
                "--release",
                "-p",
                "dmux",
                "--bin",
                "dmux",
                "--bin",
                "pane-bootstrap",
            ],
            cwd=dotfiles / "scripts/rust",
            env={"CARGO_TARGET_DIR": str(target)},
            unset_env=AMBIENT_MUX_VARS,
            log=log,
        )
        artifacts = self._copy_artifacts(
            root / "artifacts/macos/dotfiles",
            {
                "dmux": target / "release/dmux",
                "pane-bootstrap": target / "release/pane-bootstrap",
            },
        )
        release.data["artifacts"]["mac_dotfiles"] = artifacts
        self.store.checkpoint(release, "build.mac.dotfiles", {"artifacts": artifacts})

    @staticmethod
    def _mac_dmux_test_environment(release: Release, target: Path) -> dict[str, str]:
        fork = release.data["artifacts"]["mac_wezterm"]
        fork_wezterm = Path(fork["wezterm"]["path"])
        fork_mux_server = Path(fork["wezterm-mux-server"]["path"])
        for binary in (fork_wezterm, fork_mux_server):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise Refusal(f"exact fork test binary is absent or not executable: {binary}")
        return {
            "CARGO_TARGET_DIR": str(target),
            "DMUX_TEST_FORK_WEZTERM": str(fork_wezterm),
            "DMUX_TEST_FORK_MUX_SERVER": str(fork_mux_server),
            "DMUX_TEST_REQUIRE_FORK": "1",
            "PATH": f"{fork_wezterm.parent}{os.pathsep}{os.environ.get('PATH', '')}",
        }

    def _artifact_root(self, release: Release) -> Path:
        return self.config.packages_root / release.release_id

    def _log_path(self, release: Release, name: str) -> Path:
        return self.store.release_dir(release.release_id) / "logs" / name

    def _ensure_worktree(self, release: Release, source: str, path: Path) -> Path:
        repo = Path(release.data["frozen"][source]["repo"])
        commit = release.data["frozen"][source]["commit"]
        if path.exists():
            if not (path / ".git").exists():
                raise Refusal(f"worktree target already exists but is not a Git worktree: {path}")
        else:
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            self.runner.capture(
                ["git", "-C", str(repo), "worktree", "add", "--detach", str(path), commit]
            )
        actual = self.runner.capture(["git", "-C", str(path), "rev-parse", "HEAD"]).stdout.strip()
        if actual != commit:
            raise Refusal(f"worktree {path} is {actual}, expected {commit}")
        if (path / ".gitmodules").is_file():
            self.runner.stream(
                ["git", "-C", str(path), "submodule", "update", "--init", "--recursive"],
                log=self._log_path(release, f"worktree-{source}-submodules.log"),
            )
        return path

    def _require_clean_frozen_worktree(self, release: Release, source: str, path: Path) -> None:
        expected = release.data["frozen"][source]["commit"]
        actual = self.runner.capture(["git", "-C", str(path), "rev-parse", "HEAD"]).stdout.strip()
        dirty = self.runner.capture(
            ["git", "-C", str(path), "status", "--porcelain=v1", "--untracked-files=all"]
        ).stdout.strip()
        if actual != expected or dirty:
            raise Refusal(f"dirty or stale source build refused: {path}")

    @staticmethod
    def _require_artifact_dir(path: Path, *, create: bool) -> None:
        if not path.is_absolute():
            raise StateError(f"artifact directory must be absolute: {path}")
        if create:
            path.mkdir(mode=0o700, parents=True, exist_ok=True)
        metadata = path.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            raise Refusal(f"artifact path is not a real directory: {path}")
        if metadata.st_uid != os.getuid():
            raise Refusal(f"artifact directory is not owned by this user: {path}")

    def _copy_artifacts(self, output: Path, sources: dict[str, Path]) -> dict[str, Any]:
        self._require_artifact_dir(output, create=True)
        result: dict[str, Any] = {}
        for name, source in sources.items():
            if not source.is_file() or not os.access(source, os.X_OK):
                raise Refusal(f"expected executable build artifact is absent: {source}")
            target = output / name
            temporary = output / f".{name}.{uuid.uuid4().hex}.tmp"
            with open(source, "rb") as reader, open(temporary, "xb") as writer:
                shutil.copyfileobj(reader, writer, length=1024 * 1024)
                writer.flush()
                os.fsync(writer.fileno())
            os.chmod(temporary, 0o755)
            os.replace(temporary, target)
            result[name] = {
                "path": str(target),
                "sha256": sha256_file(target),
                "bytes": target.stat().st_size,
            }
        return result

    def _verify_artifact_set(self, raw: Any, label: str) -> None:
        if not isinstance(raw, dict) or not raw:
            raise StateError(f"release has no {label} artifact set")
        for name, artifact in raw.items():
            if not isinstance(artifact, dict):
                raise StateError(f"malformed {label}.{name} artifact")
            path = Path(artifact.get("path", ""))
            if not path.is_file():
                raise Refusal(f"release artifact disappeared: {path}")
            actual = sha256_file(path)
            if actual != artifact.get("sha256"):
                raise Refusal(f"release artifact hash changed: {path}")

    # Archie staging -------------------------------------------------------

    def stage_archie(self, release: Release, *, skip_tests: bool = False) -> Release:
        host = release.data["hosts"]["archie"]["ssh"]
        remote_root = self.config.archie_home / "packages/dmux-rollouts" / release.release_id
        self._remote_private_directory(host, remote_root)
        if not release.completed("stage.archie.config_preflight"):
            config_evidence = self._archie_config_preflight(release)
            release.data["rollback"]["archie"]["config"] = config_evidence
            self.store.checkpoint(release, "stage.archie.config_preflight", config_evidence)

        dotfiles = self._ensure_remote_worktree(
            host,
            self.config.archie_dotfiles_repo,
            release.data["frozen"]["dotfiles"]["commit"],
            remote_root / "worktrees/dotfiles",
            "dmux",
        )
        wezterm = self._ensure_remote_worktree(
            host,
            self.config.archie_wezterm_repo,
            release.data["frozen"]["wezterm"]["commit"],
            remote_root / "worktrees/wezterm",
            "dmux-primitives",
        )

        if not release.completed("stage.archie.dotfiles"):
            target = remote_root / "targets/dotfiles"
            if not skip_tests:
                self.runner.stream(
                    remote_argv(
                        host,
                        [
                            "env",
                            f"CARGO_TARGET_DIR={target}",
                            "cargo",
                            "test",
                            "--manifest-path",
                            str(dotfiles / "scripts/rust/Cargo.toml"),
                            "-p",
                            "dmux",
                            "--",
                            "--test-threads=1",
                        ],
                    ),
                    cwd=None,
                    env=None,
                    log=self._log_path(release, "stage-archie-dotfiles.log"),
                )
            self.runner.stream(
                remote_argv(
                    host,
                    [
                        "env",
                        f"CARGO_TARGET_DIR={target}",
                        "cargo",
                        "build",
                        "--manifest-path",
                        str(dotfiles / "scripts/rust/Cargo.toml"),
                        "--release",
                        "-p",
                        "dmux",
                        "--bin",
                        "dmux",
                        "--bin",
                        "pane-bootstrap",
                    ],
                ),
                log=self._log_path(release, "stage-archie-dotfiles.log"),
            )
            artifacts = self._remote_artifacts(
                host,
                {
                    "dmux": target / "release/dmux",
                    "pane-bootstrap": target / "release/pane-bootstrap",
                },
            )
            release.data["artifacts"]["archie_dotfiles"] = artifacts
            self.store.checkpoint(release, "stage.archie.dotfiles", {"artifacts": artifacts})
        else:
            self._verify_remote_artifacts(
                host,
                release.data["artifacts"].get("archie_dotfiles"),
                "archie_dotfiles",
            )

        if not release.completed("stage.archie.packages"):
            package_dir = remote_root / "packages"
            self._remote_private_directory(host, package_dir)
            template_path = self.config.archie_home / "packages/wezterm-fredrir-git/PKGBUILD"
            template = self.runner.capture(remote_argv(host, ["cat", str(template_path)])).stdout
            frozen_commit = release.data["frozen"]["wezterm"]["commit"]
            source_line = '"$_pkgname::git+$url.git#branch=fredrir"'
            replacement = f'"$_pkgname::git+file://{wezterm}#commit={frozen_commit}"'
            if template.count(source_line) != 1:
                raise Refusal("Archie PKGBUILD source line is not the frozen expected template")
            rendered = template.replace(source_line, replacement)
            local_template = self._artifact_root(release) / "generated/PKGBUILD.archie"
            local_template.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            self._write_generated(local_template, rendered)
            remote_temporary = package_dir / f".PKGBUILD.{release.release_id}.tmp"
            occupied = self.runner.capture(
                remote_argv(host, ["test", "-e", str(remote_temporary)]), check=False
            )
            if occupied.returncode == 0:
                remote_hash = self._remote_artifacts(host, {"PKGBUILD": remote_temporary})[
                    "PKGBUILD"
                ]["sha256"]
                if remote_hash != sha256_file(local_template):
                    raise Refusal(
                        f"remote generated-file temporary has foreign content: {remote_temporary}"
                    )
            else:
                self.runner.capture(
                    ["scp", "-q", str(local_template), f"{host}:{remote_temporary}"]
                )
            self.runner.capture(remote_argv(host, ["chmod", "0600", str(remote_temporary)]))
            self.runner.capture(
                remote_argv(
                    host,
                    ["mv", "-fT", "--", str(remote_temporary), str(package_dir / "PKGBUILD")],
                )
            )
            if not skip_tests:
                self.runner.stream(
                    remote_argv(
                        host,
                        [
                            "env",
                            f"SRCDEST={package_dir / 'srcdest'}",
                            f"CARGO_TARGET_DIR={remote_root / 'targets/wezterm-package'}",
                            "makepkg",
                            "--dir",
                            str(package_dir),
                            "--cleanbuild",
                            "--force",
                            "--noconfirm",
                        ],
                    ),
                    cwd=None,
                    log=self._log_path(release, "stage-archie-packages.log"),
                )
            else:
                self.runner.stream(
                    remote_argv(
                        host,
                        [
                            "env",
                            f"SRCDEST={package_dir / 'srcdest'}",
                            f"CARGO_TARGET_DIR={remote_root / 'targets/wezterm-package'}",
                            "makepkg",
                            "--dir",
                            str(package_dir),
                            "--cleanbuild",
                            "--force",
                            "--noconfirm",
                            "--nocheck",
                        ],
                    ),
                    cwd=None,
                    log=self._log_path(release, "stage-archie-packages.log"),
                )
            found = self.runner.capture(
                remote_argv(
                    host,
                    [
                        "find",
                        str(package_dir),
                        "-maxdepth",
                        "1",
                        "-type",
                        "f",
                        "-name",
                        "*.pkg.tar.zst",
                        "-print",
                    ],
                )
            ).stdout.splitlines()
            packages = self._classify_arch_packages(host, found, frozen_commit)
            release.data["artifacts"]["archie_packages"] = packages
            self.store.checkpoint(release, "stage.archie.packages", {"packages": packages})
        else:
            self._verify_remote_artifacts(
                host,
                release.data["artifacts"].get("archie_packages"),
                "archie_packages",
            )
        if not release.data["rollback"]["archie"].get("packages"):
            release.data["rollback"]["archie"]["packages"] = self._archie_rollback_packages(host)
        release.data["hosts"]["archie"]["stage_root"] = str(remote_root)
        release.set_phase("archie_staged")
        self.store.save(release)
        return release

    def archie_install_command(self, release: Release) -> str:
        packages = release.data["artifacts"].get("archie_packages")
        if not isinstance(packages, dict) or set(packages) != {"main", "debug"}:
            raise StateError("Archie packages are not staged")
        import shlex

        paths = [packages["main"]["path"], packages["debug"]["path"]]
        inner = shlex.join(["sudo", "pacman", "-U", *paths])
        return shlex.join(["ssh", "-t", release.data["hosts"]["archie"]["ssh"], inner])

    def _remote_private_directory(self, host: str, path: Path) -> None:
        if not path.is_absolute():
            raise StateError(f"remote staging path must be absolute: {path}")
        self.runner.capture(remote_argv(host, ["install", "-d", "-m", "0700", str(path)]))
        metadata = self.runner.capture(
            remote_argv(host, ["stat", "-c", "%U:%a:%F", str(path)])
        ).stdout.strip()
        if metadata != "fredrir:700:directory":
            raise Refusal(f"remote staging directory is not private: {path} ({metadata})")

    def _ensure_remote_worktree(
        self,
        host: str,
        repo: Path,
        commit: str,
        path: Path,
        branch: str,
    ) -> Path:
        self.runner.capture(remote_argv(host, ["git", "-C", str(repo), "fetch", "origin", branch]))
        self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "cat-file", "-e", f"{commit}^{{commit}}"])
        )
        exists = self.runner.capture(remote_argv(host, ["test", "-e", str(path)]), check=False)
        if exists.returncode == 0:
            valid = self.runner.capture(
                remote_argv(host, ["test", "-e", str(path / ".git")]), check=False
            )
            if valid.returncode != 0:
                raise Refusal(f"remote worktree target is occupied: {path}")
        else:
            self._remote_private_directory(host, path.parent)
            self.runner.capture(
                remote_argv(
                    host,
                    ["git", "-C", str(repo), "worktree", "add", "--detach", str(path), commit],
                )
            )
        actual = self.runner.capture(
            remote_argv(host, ["git", "-C", str(path), "rev-parse", "HEAD"])
        ).stdout.strip()
        dirty = self.runner.capture(
            remote_argv(
                host,
                ["git", "-C", str(path), "status", "--porcelain=v1", "--untracked-files=all"],
            )
        ).stdout.strip()
        if actual != commit or dirty:
            raise Refusal(f"dirty or stale remote source build refused: {path}")
        return path

    def _remote_artifacts(self, host: str, paths: dict[str, Path]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for name, path in paths.items():
            raw = self.runner.capture(
                remote_argv(host, ["sha256sum", "--", str(path)])
            ).stdout.strip()
            digest, _, reported = raw.partition("  ")
            if reported != str(path) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise Refusal(f"unexpected remote hash response for {path}: {raw}")
            size = self.runner.capture(
                remote_argv(host, ["stat", "-c", "%s", str(path)])
            ).stdout.strip()
            result[name] = {"path": str(path), "sha256": digest, "bytes": int(size)}
        return result

    def _verify_remote_artifacts(self, host: str, raw: Any, label: str) -> None:
        if not isinstance(raw, dict) or not raw:
            raise StateError(f"release has no {label} artifacts")
        expected = {name: Path(row["path"]) for name, row in raw.items()}
        actual = self._remote_artifacts(host, expected)
        for name, row in raw.items():
            if actual[name]["sha256"] != row.get("sha256"):
                raise Refusal(f"remote staged artifact hash changed: {row['path']}")

    def _classify_arch_packages(
        self, host: str, paths: Iterable[str], frozen_commit: str
    ) -> dict[str, Any]:
        selected: dict[str, Path] = {}
        for text in paths:
            path = Path(text)
            match = PACKAGE_RE.fullmatch(path.name)
            if match is None or frozen_commit[:8] not in match.group("version"):
                continue
            key = "debug" if match.group("debug") else "main"
            if key in selected:
                raise Refusal(f"multiple staged Archie {key} packages match the frozen commit")
            selected[key] = path
        if set(selected) != {"main", "debug"}:
            raise Refusal("makepkg did not produce exactly one main and one debug package")
        for path in selected.values():
            self.runner.capture(remote_argv(host, ["bsdtar", "-tf", str(path)]))
        return self._remote_artifacts(host, selected)

    def _archie_rollback_packages(self, host: str) -> dict[str, Any]:
        installed: dict[str, Any] = {}
        for package in ("wezterm-fredrir-git", "wezterm-fredrir-git-debug"):
            query = self.runner.capture(remote_argv(host, ["pacman", "-Q", package]), check=False)
            if query.returncode != 0:
                raise Refusal(f"Archie rollback package is not installed: {package}")
            words = query.stdout.strip().split()
            if len(words) != 2 or words[0] != package:
                raise Refusal(f"unexpected pacman query output for {package}")
            version = words[1]
            cache = self.config.archie_home / ".cache/yay" / "wezterm-fredrir-git"
            name = f"{package}-{version}-x86_64.pkg.tar.zst"
            path = cache / name
            test = self.runner.capture(remote_argv(host, ["test", "-f", str(path)]), check=False)
            if test.returncode != 0:
                raise Refusal(f"exact Archie rollback archive is absent: {path}")
            row = self._remote_artifacts(host, {package: path})[package]
            row["version"] = version
            installed[package] = row
        return installed

    @staticmethod
    def _write_generated(path: Path, content: str) -> None:
        temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
        with open(temporary, "x", encoding="utf-8") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)

    # Mac deployment -------------------------------------------------------

    def deploy_mac(self, release: Release, *, approved_spaces: set[str] | None = None) -> Release:
        approved = set(approved_spaces or set())
        if release.data["smoke"].get("space_uid"):
            approved.add(release.data["smoke"]["space_uid"])
        self._require_live_config_matches(release)
        self._verify_artifact_set(release.data["artifacts"].get("mac_dotfiles"), "mac_dotfiles")
        self._verify_artifact_set(release.data["artifacts"].get("mac_wezterm"), "mac_wezterm")

        before = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
        retired = self._retire_empty_managed_guis()
        before = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=True)
        if retired:
            before["retired_empty_gui_processes"] = retired
        if not release.completed("deploy.mac.preflight"):
            self.store.checkpoint(release, "deploy.mac.preflight", before)
        else:
            frozen = release.checkpoints["deploy.mac.preflight"]["evidence"]
            if frozen.get("backend_instance_uid") != before["backend_instance_uid"]:
                raise Refusal("Mac backend instance changed since deployment preflight")

        if not release.completed("deploy.mac.backup"):
            backups = self._backup_mac_files(release)
            release.data["rollback"]["mac"]["files"] = backups
            release.data["rollback"]["mac"]["launchd_dmux_wez_first"] = self.runner.capture(
                ["launchctl", "getenv", "DMUX_WEZ_FIRST"], check=False
            ).stdout.strip()
            self.store.checkpoint(release, "deploy.mac.backup", {"files": backups})
        else:
            self._verify_mac_backups(release)

        if not release.completed("deploy.mac.install"):
            try:
                self._install_mac_release(release)
            except BaseException:
                self._restore_mac_files(release, require_release_hash=False)
                self.runner.capture(
                    ["codesign", "--force", "--deep", "--sign", "-", str(self.config.mac_app)],
                    check=False,
                )
                raise
            evidence = self._installed_mac_hashes(release)
            release.data["artifacts"]["mac_installed"] = evidence
            self.store.checkpoint(release, "deploy.mac.install", evidence)
        else:
            self._verify_installed_mac_hashes(release)

        if not release.completed("deploy.mac.service"):
            if not release.completed("deploy.mac.service.intent"):
                self.store.checkpoint(release, "deploy.mac.service.intent", before)
            intent = release.checkpoints["deploy.mac.service.intent"]["evidence"]
            current = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=True)
            if current["pid"] != intent["pid"] and current["epoch"] != intent["epoch"]:
                after = current
            else:
                self.runner.capture(["launchctl", "setenv", "DMUX_WEZ_FIRST", "1"])
                label = release.data["hosts"]["mac"]["service_label"]
                self.runner.capture(
                    ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{label}"],
                    timeout=60,
                )
                after = self._wait_mac_owner(
                    lambda row: row["pid"] != intent["pid"] and row["epoch"] != intent["epoch"],
                    approved_spaces=approved,
                    require_quiet=True,
                    timeout=90,
                )
            self.store.checkpoint(release, "deploy.mac.service", after)
        else:
            after = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=True)
            frozen = release.checkpoints["deploy.mac.service"]["evidence"]
            if after["backend_instance_uid"] != frozen["backend_instance_uid"]:
                raise Refusal("Mac service now belongs to a different backend instance")
        release.set_phase("mac_deployed")
        self.store.save(release)
        return release

    def _require_live_config_matches(self, release: Release) -> None:
        repo = self.config.dotfiles_repo
        commit = release.data["frozen"]["dotfiles"]["commit"]
        head = self.runner.capture(["git", "-C", str(repo), "rev-parse", "HEAD"]).stdout.strip()
        if head != commit:
            raise Refusal(f"live config checkout moved from frozen commit {commit} to {head}")
        managed_paths = [
            "shared/wezterm",
            "shared/tmux",
            "shared/zsh/conf.d/94-dmux-context.zsh",
            "macos/launchd/com.fredrir.wezterm-mux.plist",
            "linux/arch/wezterm-mux",
            "scripts/rust/crates/dmux",
        ]
        diff = self.runner.capture(
            ["git", "-C", str(repo), "status", "--porcelain=v1", "--", *managed_paths]
        ).stdout.strip()
        if diff:
            raise Refusal("managed rollout sources are dirty; exact live config cannot be proven")

    def _retire_empty_managed_guis(self) -> list[dict[str, Any]]:
        instances = self._mac_runtime_dir() / "bridge/instances"
        if not instances.is_dir():
            return []
        live: dict[int, dict[str, Any]] = {}
        for path in instances.glob("*/heartbeat.json"):
            try:
                heartbeat = self._load_bounded_json(path, maximum=1024 * 1024)
                self._require_live_heartbeat(heartbeat)
            except RolloutError:
                continue
            pid = heartbeat["pid"]
            if heartbeat.get("panes"):
                raise Refusal(
                    f"managed GUI PID {pid} still owns visible panes; deployment will not stop it"
                )
            for row in heartbeat.get("domains", {}).values():
                if not isinstance(row, dict) or row.get("backend_instance_uid") is None:
                    continue
                if (
                    row.get("state") != "Detached"
                    or row.get("pane_count") != 0
                    or row.get("system_pane_count") != 0
                ):
                    raise Refusal(f"managed GUI PID {pid} still has an attached persistent domain")
            live[pid] = heartbeat
        retired = []
        for pid, heartbeat in sorted(live.items()):
            self._require_live_heartbeat(heartbeat)
            os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                result = self.runner.capture(["ps", "-p", str(pid), "-o", "pid="], check=False)
                if result.returncode != 0 or not result.stdout.strip():
                    break
                time.sleep(0.1)
            else:
                raise Refusal(f"exact empty managed GUI PID {pid} ignored SIGTERM")
            retired.append(
                {
                    "pid": pid,
                    "process_start_token": heartbeat["process_start_token"],
                    "gui_instance": heartbeat["gui_instance"],
                }
            )
        return retired

    def _backup_mac_files(self, release: Release) -> list[dict[str, Any]]:
        backup = self._artifact_root(release) / "rollback/macos"
        self._require_artifact_dir(backup, create=True)
        result = []
        for target in self._mac_targets():
            if not target.is_file() or target.is_symlink():
                raise Refusal(f"Mac install target must be a real file: {target}")
            destination = backup / target.name
            if destination.exists():
                if not destination.is_file() or sha256_file(destination) != sha256_file(target):
                    raise Refusal(f"untracked rollback file already exists: {destination}")
            else:
                self._copy_one(target, destination, mode=0o755)
            result.append(
                {
                    "target": str(target),
                    "backup": str(destination),
                    "before_sha256": sha256_file(destination),
                }
            )
        return result

    def _mac_targets(self) -> list[Path]:
        macos = self.config.mac_app / "Contents/MacOS"
        return [
            self.config.mac_dmux,
            self.config.mac_pane_bootstrap,
            macos / "wezterm",
            macos / "wezterm-gui",
            macos / "wezterm-mux-server",
        ]

    def _verify_mac_backups(self, release: Release) -> None:
        rows = release.data["rollback"]["mac"].get("files")
        if not isinstance(rows, list) or len(rows) != 5:
            raise StateError("Mac rollback inventory is incomplete")
        for row in rows:
            path = Path(row["backup"])
            if not path.is_file() or sha256_file(path) != row["before_sha256"]:
                raise Refusal(f"Mac rollback artifact is missing or changed: {path}")

    def _install_mac_release(self, release: Release) -> None:
        dotfiles = release.data["artifacts"]["mac_dotfiles"]
        wezterm = release.data["artifacts"]["mac_wezterm"]
        mapping = {
            self.config.mac_dmux: Path(dotfiles["dmux"]["path"]),
            self.config.mac_pane_bootstrap: Path(dotfiles["pane-bootstrap"]["path"]),
            self.config.mac_app / "Contents/MacOS/wezterm": Path(wezterm["wezterm"]["path"]),
            self.config.mac_app / "Contents/MacOS/wezterm-gui": Path(
                wezterm["wezterm-gui"]["path"]
            ),
            self.config.mac_app / "Contents/MacOS/wezterm-mux-server": Path(
                wezterm["wezterm-mux-server"]["path"]
            ),
        }
        for target, source in mapping.items():
            self._copy_one(source, target, mode=0o755)
        self.runner.capture(
            ["codesign", "--force", "--deep", "--sign", "-", str(self.config.mac_app)],
            timeout=120,
        )
        self.runner.capture(
            ["codesign", "--verify", "--deep", "--strict", str(self.config.mac_app)]
        )
        version = self.runner.capture(
            [str(self.config.mac_app / "Contents/MacOS/wezterm"), "--version"]
        ).stdout.strip()
        expected = release.data["frozen"]["wezterm"]["commit"][:8]
        if expected not in version:
            raise Refusal(f"installed Mac WezTerm does not report {expected}: {version}")

    def _installed_mac_hashes(self, release: Release) -> dict[str, Any]:
        rows = {}
        for target in self._mac_targets():
            rows[target.name] = {
                "path": str(target),
                "sha256": sha256_file(target),
                "bytes": target.stat().st_size,
            }
        rows["codesign"] = self.runner.capture(
            ["codesign", "-dvv", str(self.config.mac_app)], check=True
        ).stderr.strip()
        return rows

    def _verify_installed_mac_hashes(self, release: Release) -> None:
        rows = release.data["artifacts"].get("mac_installed")
        if not isinstance(rows, dict):
            raise StateError("Mac installed hashes are absent")
        for name, row in rows.items():
            if name == "codesign":
                continue
            path = Path(row["path"])
            if not path.is_file() or sha256_file(path) != row["sha256"]:
                raise Refusal(f"installed Mac release drifted: {path}")
        self.runner.capture(
            ["codesign", "--verify", "--deep", "--strict", str(self.config.mac_app)]
        )

    @staticmethod
    def _copy_one(source: Path, target: Path, *, mode: int) -> None:
        if not source.is_file():
            raise Refusal(f"copy source is absent: {source}")
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary = target.parent / f".{target.name}.{uuid.uuid4().hex}.tmp"
        flags = os.O_CREAT | os.O_EXCL | os.O_WRONLY | getattr(os, "O_CLOEXEC", 0) | os.O_NOFOLLOW
        fd = os.open(temporary, flags, mode)
        try:
            with open(source, "rb") as reader, os.fdopen(fd, "wb", closefd=False) as writer:
                shutil.copyfileobj(reader, writer, length=1024 * 1024)
                writer.flush()
                os.fsync(writer.fileno())
            os.chmod(temporary, mode)
            os.replace(temporary, target)
            dir_fd = os.open(target.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
            try:
                os.fsync(dir_fd)
            finally:
                os.close(dir_fd)
        finally:
            os.close(fd)
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass

    def _restore_mac_files(self, release: Release, *, require_release_hash: bool) -> None:
        self._verify_mac_backups(release)
        installed = release.data["artifacts"].get("mac_installed", {})
        for row in release.data["rollback"]["mac"]["files"]:
            target = Path(row["target"])
            if require_release_hash:
                expected = next(
                    (
                        value["sha256"]
                        for value in installed.values()
                        if isinstance(value, dict) and value.get("path") == str(target)
                    ),
                    None,
                )
                if expected is None or not target.is_file() or sha256_file(target) != expected:
                    raise Refusal(f"rollback target was modified after deployment: {target}")
            self._copy_one(Path(row["backup"]), target, mode=0o755)

    def _mac_runtime_dir(self) -> Path:
        base = self.runner.capture(["getconf", "DARWIN_USER_TEMP_DIR"]).stdout.strip()
        if not base.startswith("/") or not base.endswith("/"):
            raise Refusal("getconf returned an invalid Darwin user temporary directory")
        return Path(base) / "dmux"

    def _mac_owner_snapshot(
        self, *, approved_spaces: set[str], require_quiet: bool
    ) -> dict[str, Any]:
        runtime = self._mac_runtime_dir()
        descriptor = self._load_bounded_json(runtime / "wez-dmux.json", maximum=65536)
        required = {
            "state",
            "epoch",
            "pid",
            "socket",
            "socket_dev",
            "socket_ino",
            "start_token",
            "backend_instance_uid",
            "sentinel_window_id",
            "sentinel_tab_id",
            "sentinel_pane_id",
        }
        if not isinstance(descriptor, dict) or not required.issubset(descriptor):
            raise Refusal("Mac service descriptor is incomplete")
        if descriptor["state"] != "ready":
            raise Refusal(f"Mac owner is not ready: {descriptor['state']}")
        socket = Path(descriptor["socket"])
        if socket != runtime / "wez-dmux.sock":
            raise Refusal(f"Mac owner descriptor names a non-fixed socket: {socket}")
        process = self.runner.capture(
            ["ps", "-p", str(descriptor["pid"]), "-o", "pid=", "-o", "lstart=", "-o", "comm="],
            check=False,
        )
        if process.returncode != 0 or "wezterm-mux-server" not in process.stdout:
            raise Refusal("Mac descriptor PID is not the exact live mux server")
        rows = self._wez_cli_json(socket, "list")
        clients = self._wez_cli_json(socket, "list-clients")
        if require_quiet and clients:
            raise Refusal(
                "Mac owner has attached clients; deployment requires an explicit quiet point"
            )
        inventory = self._validate_native_inventory(rows, descriptor, approved_spaces)
        recovery = self._dmux_json(["recovery", "status", "--format", "json"])
        result = recovery.get("result", {}) if isinstance(recovery, dict) else {}
        status = result.get("status", {}) if isinstance(result, dict) else {}
        if (
            recovery.get("ok") is not True
            or status.get("state") != "ready"
            or status.get("server_epoch") != descriptor["epoch"]
            or status.get("backend_instance_uid") != descriptor["backend_instance_uid"]
        ):
            raise Refusal("Mac recovery status does not match the live ready owner")
        return {
            "pid": descriptor["pid"],
            "process": process.stdout.strip(),
            "epoch": descriptor["epoch"],
            "backend_instance_uid": descriptor["backend_instance_uid"],
            "socket": str(socket),
            "socket_dev": descriptor["socket_dev"],
            "socket_ino": descriptor["socket_ino"],
            "start_token": descriptor["start_token"],
            "sentinel": inventory["sentinel"],
            "spaces": inventory["spaces"],
            "clients": clients,
            "recovery_generation": status.get("generation_uid"),
            "recovery_manifest": status.get("manifest_id"),
        }

    def _wait_mac_owner(
        self,
        predicate: Callable[[dict[str, Any]], bool],
        *,
        approved_spaces: set[str],
        require_quiet: bool,
        timeout: float,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        last: Exception | None = None
        while time.monotonic() < deadline:
            try:
                row = self._mac_owner_snapshot(
                    approved_spaces=approved_spaces, require_quiet=require_quiet
                )
                if predicate(row):
                    return row
            except RolloutError as error:
                last = error
            time.sleep(0.25)
        raise Refusal(f"Mac service did not reach the requested postcondition: {last}")

    def _wez_cli_json(self, socket: Path, command: str) -> Any:
        binary = self.config.mac_app / "Contents/MacOS/wezterm"
        output = self.runner.capture(
            [str(binary), "cli", "--no-auto-start", "--prefer-mux", command, "--format", "json"],
            env={"WEZTERM_UNIX_SOCKET": str(socket)},
            unset_env=AMBIENT_MUX_VARS,
            timeout=30,
        ).stdout
        try:
            return json.loads(output)
        except json.JSONDecodeError as error:
            raise Refusal(f"WezTerm {command} returned malformed JSON") from error

    def _dmux_json(self, argv: Sequence[str]) -> Any:
        output = self.runner.capture(
            [str(self.config.mac_dmux), *argv],
            env={"DMUX_WEZ_FIRST": "1"},
            unset_env=AMBIENT_MUX_VARS,
            timeout=40,
        ).stdout
        try:
            return json.loads(output)
        except json.JSONDecodeError as error:
            raise Refusal(f"dmux {' '.join(argv)} returned malformed JSON") from error

    @staticmethod
    def _validate_native_inventory(
        rows: Any, descriptor: dict[str, Any], approved_spaces: set[str]
    ) -> dict[str, Any]:
        if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
            raise Refusal("owner pane inventory is not a JSON array")
        sentinel = []
        spaces: dict[str, list[int]] = {}
        for row in rows:
            workspace = row.get("workspace")
            system = SYSTEM_WORKSPACE_RE.fullmatch(workspace or "")
            if system:
                if system.group("epoch") != descriptor["epoch"]:
                    raise Refusal("owner contains a stale or foreign sentinel epoch")
                sentinel.append((row.get("window_id"), row.get("tab_id"), row.get("pane_id")))
                continue
            match = WORKSPACE_RE.fullmatch(workspace or "")
            if match is None:
                raise Refusal(f"owner has an unmanaged or malformed workspace: {workspace!r}")
            space_uid = require_space_uid(match.group("space"))
            if space_uid not in approved_spaces:
                raise Refusal(
                    f"unexpected live user Space {space_uid}; rerun with --approve-space {space_uid}"
                )
            spaces.setdefault(space_uid, []).append(row.get("pane_id"))
        expected_sentinel = (
            descriptor["sentinel_window_id"],
            descriptor["sentinel_tab_id"],
            descriptor["sentinel_pane_id"],
        )
        if sentinel != [expected_sentinel]:
            raise Refusal(f"owner must contain exactly its recorded sentinel: {sentinel}")
        return {
            "sentinel": {
                "window_id": expected_sentinel[0],
                "tab_id": expected_sentinel[1],
                "pane_id": expected_sentinel[2],
            },
            "spaces": {key: sorted(value) for key, value in sorted(spaces.items())},
        }

    @staticmethod
    def _load_bounded_json(path: Path, *, maximum: int) -> Any:
        flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | os.O_NOFOLLOW
        try:
            fd = os.open(path, flags)
        except OSError as error:
            raise Refusal(f"cannot open required JSON evidence {path}: {error}") from error
        try:
            metadata = os.fstat(fd)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid():
                raise Refusal(f"JSON evidence is not a current-user regular file: {path}")
            payload = os.read(fd, maximum + 1)
            if len(payload) > maximum:
                raise Refusal(f"JSON evidence exceeds {maximum} bytes: {path}")
            return json.loads(payload.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise Refusal(f"malformed JSON evidence: {path}") from error
        finally:
            os.close(fd)

    # Archie resume/deploy -------------------------------------------------

    def resume(self, release: Release, *, approved_spaces: set[str] | None = None) -> str:
        if not release.completed("stage.archie.packages"):
            raise Refusal("Archie packages have not been staged")
        host = release.data["hosts"]["archie"]["ssh"]
        approved = set(approved_spaces or set())
        if release.data["smoke"].get("space_uid"):
            approved.add(release.data["smoke"]["space_uid"])
        if not self._archie_packages_installed(release):
            self.store.append_event(
                release,
                "awaiting_archie_pacman",
                {"command": self.archie_install_command(release)},
            )
            return "awaiting_archie_pacman"
        if not release.completed("deploy.archie.packages"):
            packages = {
                name: self.runner.capture(remote_argv(host, ["pacman", "-Q", name])).stdout.strip()
                for name in ("wezterm-fredrir-git", "wezterm-fredrir-git-debug")
            }
            self.store.checkpoint(release, "deploy.archie.packages", packages)

        if not release.completed("deploy.archie.config"):
            evidence = self._deploy_archie_config(release)
            self.store.checkpoint(release, "deploy.archie.config", evidence)

        if not release.completed("deploy.archie.user_backup"):
            backups = self._backup_archie_user_binaries(release)
            release.data["rollback"]["archie"]["user_files"] = backups
            environment = self.runner.capture(
                remote_argv(host, ["systemctl", "--user", "show-environment"])
            ).stdout.splitlines()
            release.data["rollback"]["archie"]["dmux_wez_first"] = next(
                (
                    line.partition("=")[2]
                    for line in environment
                    if line.startswith("DMUX_WEZ_FIRST=")
                ),
                "",
            )
            self.store.checkpoint(release, "deploy.archie.user_backup", {"files": backups})
        else:
            self._verify_remote_artifacts(
                host,
                {
                    Path(row["target"]).name: {
                        "path": row["backup"],
                        "sha256": row["before_sha256"],
                    }
                    for row in release.data["rollback"]["archie"]["user_files"]
                },
                "archie_user_backups",
            )

        before: dict[str, Any] | None = None
        if not release.completed("deploy.archie.service"):
            try:
                before = self._archie_owner_snapshot(
                    release, approved_spaces=approved, require_quiet=True
                )
            except RolloutError:
                before = None
        if not release.completed("deploy.archie.user_install"):
            installed = self._install_archie_user_binaries(release)
            release.data["artifacts"]["archie_user_installed"] = installed
            self.store.checkpoint(release, "deploy.archie.user_install", installed)
        else:
            self._verify_remote_artifacts(
                host,
                release.data["artifacts"].get("archie_user_installed"),
                "archie_user_installed",
            )

        if not release.completed("deploy.archie.service"):
            if not release.completed("deploy.archie.service.intent"):
                self.store.checkpoint(
                    release, "deploy.archie.service.intent", before or {"pid": None, "epoch": None}
                )
            intent = release.checkpoints["deploy.archie.service.intent"]["evidence"]
            try:
                current = self._archie_owner_snapshot(
                    release, approved_spaces=approved, require_quiet=True
                )
            except RolloutError:
                current = None
            if (
                current is not None
                and current["pid"] != intent.get("pid")
                and current["epoch"] != intent.get("epoch")
            ):
                after = current
            else:
                self.runner.capture(
                    remote_argv(
                        host, ["systemctl", "--user", "set-environment", "DMUX_WEZ_FIRST=1"]
                    )
                )
                self.runner.capture(remote_argv(host, ["systemctl", "--user", "daemon-reload"]))
                self.runner.capture(
                    remote_argv(
                        host, ["systemctl", "--user", "restart", "wezterm-mux-server.service"]
                    ),
                    timeout=60,
                )
                after = self._wait_archie_owner(
                    release,
                    lambda row: (
                        row["pid"] != intent.get("pid") and row["epoch"] != intent.get("epoch")
                    ),
                    approved_spaces=approved,
                    require_quiet=True,
                    timeout=90,
                )
            self.store.checkpoint(release, "deploy.archie.service", after)
        else:
            self._archie_owner_snapshot(release, approved_spaces=approved, require_quiet=True)
        release.set_phase("deployed")
        self.store.save(release)
        return "deployed"

    def _archie_packages_installed(self, release: Release) -> bool:
        host = release.data["hosts"]["archie"]["ssh"]
        packages = release.data["artifacts"]["archie_packages"]
        for name, key in (("wezterm-fredrir-git", "main"), ("wezterm-fredrir-git-debug", "debug")):
            expected_name = Path(packages[key]["path"]).name
            match = PACKAGE_RE.fullmatch(expected_name)
            assert match is not None
            expected = f"{match.group('version')}-1"
            result = self.runner.capture(remote_argv(host, ["pacman", "-Q", name]), check=False)
            if result.returncode != 0 or result.stdout.strip() != f"{name} {expected}":
                return False
        return True

    def _archie_config_preflight(self, release: Release) -> dict[str, Any]:
        host = release.data["hosts"]["archie"]["ssh"]
        repo = self.config.archie_dotfiles_repo
        frozen = release.data["frozen"]["dotfiles"]["commit"]
        self.runner.capture(remote_argv(host, ["git", "-C", str(repo), "fetch", "origin", "dmux"]))
        branch = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "branch", "--show-current"])
        ).stdout.strip()
        if branch != "dmux":
            raise Refusal(f"Archie live config must be on branch dmux, found {branch!r}")
        head = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "rev-parse", "HEAD"])
        ).stdout.strip()
        require_commit(head, "Archie pre-release config commit")
        ancestor = self.runner.capture(
            remote_argv(
                host, ["git", "-C", str(repo), "merge-base", "--is-ancestor", head, frozen]
            ),
            check=False,
        )
        if ancestor.returncode != 0:
            raise Refusal("Archie live config cannot fast-forward to the frozen release")
        cached = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "diff", "--cached", "--quiet"]),
            check=False,
        )
        if cached.returncode != 0:
            raise Refusal(
                "Archie live config has staged changes; rollout will not rewrite its index"
            )
        dirty = self._remote_git_dirty(host, repo)
        changed_raw = self.runner.capture(
            remote_argv(
                host,
                ["git", "-C", str(repo), "diff", "--name-status", "--no-renames", head, frozen],
            )
        ).stdout.splitlines()
        changed = []
        for line in changed_raw:
            status_code, separator, path = line.partition("\t")
            if separator != "\t" or status_code not in {"A", "M", "D"} or not path:
                raise Refusal(f"unsupported Archie release path transition: {line!r}")
            changed.append({"status": status_code, "path": path})
        dirty_paths = set(self._remote_git_dirty_paths(host, repo))
        overlap = dirty_paths & {row["path"] for row in changed}
        if overlap:
            raise Refusal(f"Archie dirty files overlap the frozen release: {sorted(overlap)}")
        managed = self.runner.capture(
            remote_argv(
                host,
                [
                    "git",
                    "-C",
                    str(repo),
                    "status",
                    "--porcelain=v1",
                    "--",
                    "shared/wezterm",
                    "shared/tmux",
                    "shared/zsh/conf.d/94-dmux-context.zsh",
                    "linux/arch/wezterm-mux",
                    "scripts/rust/crates/dmux",
                ],
            )
        ).stdout.strip()
        if managed:
            raise Refusal("Archie managed rollout sources are dirty")
        return {
            "repo": str(repo),
            "branch": branch,
            "head": head,
            "release_head": frozen,
            "dirty": dirty,
            "changed": changed,
        }

    def _deploy_archie_config(self, release: Release) -> dict[str, Any]:
        host = release.data["hosts"]["archie"]["ssh"]
        config = release.data["rollback"]["archie"].get("config")
        if not isinstance(config, dict):
            raise StateError("Archie config rollback witness is absent")
        repo = Path(config["repo"])
        old = config["head"]
        frozen = config["release_head"]
        current = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "rev-parse", "HEAD"])
        ).stdout.strip()
        if current == old:
            self.runner.capture(
                remote_argv(host, ["git", "-C", str(repo), "merge", "--ff-only", frozen])
            )
        elif current != frozen:
            raise Refusal(f"Archie config moved to an unjournaled commit: {current}")
        actual = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "rev-parse", "HEAD"])
        ).stdout.strip()
        dirty = self._remote_git_dirty(host, repo)
        if actual != frozen or dirty != config["dirty"]:
            raise Refusal("Archie config fast-forward did not preserve its pre-existing dirt")
        return {"before": old, "after": actual, "dirty_preserved": dirty}

    def _remote_git_dirty(self, host: str, repo: Path) -> list[str]:
        return self.runner.capture(
            remote_argv(
                host,
                ["git", "-C", str(repo), "status", "--porcelain=v1", "--untracked-files=all"],
            )
        ).stdout.splitlines()

    def _remote_git_dirty_paths(self, host: str, repo: Path) -> list[str]:
        paths = []
        for argv in (
            ["git", "-C", str(repo), "diff", "--name-only"],
            ["git", "-C", str(repo), "diff", "--cached", "--name-only"],
            ["git", "-C", str(repo), "ls-files", "--others", "--exclude-standard"],
        ):
            paths.extend(self.runner.capture(remote_argv(host, argv)).stdout.splitlines())
        return sorted(set(paths))

    def _backup_archie_user_binaries(self, release: Release) -> list[dict[str, Any]]:
        host = release.data["hosts"]["archie"]["ssh"]
        stage = Path(release.data["hosts"]["archie"]["stage_root"])
        backup = stage / "rollback/user"
        self._remote_private_directory(host, backup)
        rows = []
        for name in ("dmux", "pane-bootstrap"):
            target = self.config.archie_home / ".local/bin" / name
            destination = backup / name
            occupied = self.runner.capture(
                remote_argv(host, ["test", "-e", str(destination)]), check=False
            )
            if occupied.returncode == 0:
                current = self._remote_artifacts(host, {name: target})[name]["sha256"]
                saved = self._remote_artifacts(host, {name: destination})[name]["sha256"]
                if current != saved:
                    raise Refusal(f"untracked Archie rollback file already exists: {destination}")
            else:
                self.runner.capture(
                    remote_argv(host, ["cp", "--reflink=auto", "--", str(target), str(destination)])
                )
                self.runner.capture(remote_argv(host, ["chmod", "0755", str(destination)]))
            evidence = self._remote_artifacts(host, {name: destination})[name]
            rows.append(
                {
                    "target": str(target),
                    "backup": str(destination),
                    "before_sha256": evidence["sha256"],
                }
            )
        return rows

    def _install_archie_user_binaries(self, release: Release) -> dict[str, Any]:
        host = release.data["hosts"]["archie"]["ssh"]
        artifacts = release.data["artifacts"]["archie_dotfiles"]
        targets: dict[str, Path] = {}
        for name in ("dmux", "pane-bootstrap"):
            source = Path(artifacts[name]["path"])
            actual = self._remote_artifacts(host, {name: source})[name]
            if actual["sha256"] != artifacts[name]["sha256"]:
                raise Refusal(f"Archie staged user binary changed: {source}")
            target = self.config.archie_home / ".local/bin" / name
            temporary = target.parent / f".{name}.{release.release_id}.tmp"
            self.runner.capture(
                remote_argv(host, ["install", "-m", "0755", "--", str(source), str(temporary)])
            )
            self.runner.capture(remote_argv(host, ["mv", "-fT", "--", str(temporary), str(target)]))
            targets[name] = target
        installed = self._remote_artifacts(host, targets)
        for name, row in installed.items():
            if row["sha256"] != artifacts[name]["sha256"]:
                raise Refusal(f"Archie installed binary differs after atomic replacement: {name}")
        return installed

    def _archie_owner_snapshot(
        self, release: Release, *, approved_spaces: set[str], require_quiet: bool
    ) -> dict[str, Any]:
        host = release.data["hosts"]["archie"]["ssh"]
        runtime = Path("/run/user/1000/dmux")
        descriptor_raw = self.runner.capture(
            remote_argv(host, ["cat", str(runtime / "wez-dmux.json")])
        ).stdout
        try:
            descriptor = json.loads(descriptor_raw)
        except json.JSONDecodeError as error:
            raise Refusal("Archie service descriptor is malformed") from error
        if descriptor.get("state") != "ready" or descriptor.get("socket") != str(
            runtime / "wez-dmux.sock"
        ):
            raise Refusal("Archie service descriptor is not fixed and ready")
        process = self.runner.capture(
            remote_argv(
                host,
                [
                    "ps",
                    "-p",
                    str(descriptor.get("pid")),
                    "-o",
                    "pid=",
                    "-o",
                    "lstart=",
                    "-o",
                    "comm=",
                ],
            ),
            check=False,
        )
        if process.returncode != 0 or "wezterm-mux-s" not in process.stdout:
            raise Refusal("Archie descriptor PID is not the live mux server")
        env = scrubbed_env(f"WEZTERM_UNIX_SOCKET={descriptor['socket']}")
        wezterm = "/usr/bin/wezterm"
        rows = self._remote_json(
            host,
            [*env, wezterm, "cli", "--no-auto-start", "--prefer-mux", "list", "--format", "json"],
        )
        clients = self._remote_json(
            host,
            [
                *env,
                wezterm,
                "cli",
                "--no-auto-start",
                "--prefer-mux",
                "list-clients",
                "--format",
                "json",
            ],
        )
        if require_quiet and clients:
            raise Refusal("Archie owner has attached clients; deployment requires a quiet point")
        inventory = self._validate_native_inventory(rows, descriptor, approved_spaces)
        recovery = self._remote_json(
            host,
            [
                *scrubbed_env("DMUX_WEZ_FIRST=1"),
                str(self.config.archie_home / ".local/bin/dmux"),
                "recovery",
                "status",
                "--format",
                "json",
            ],
        )
        status = recovery.get("result", {}).get("status", {})
        if (
            recovery.get("ok") is not True
            or status.get("state") != "ready"
            or status.get("server_epoch") != descriptor.get("epoch")
        ):
            raise Refusal("Archie recovery status does not match the ready owner")
        return {
            "pid": descriptor["pid"],
            "process": process.stdout.strip(),
            "epoch": descriptor["epoch"],
            "backend_instance_uid": descriptor["backend_instance_uid"],
            "socket": descriptor["socket"],
            "socket_dev": descriptor["socket_dev"],
            "socket_ino": descriptor["socket_ino"],
            "start_token": descriptor["start_token"],
            "sentinel": inventory["sentinel"],
            "spaces": inventory["spaces"],
            "clients": clients,
            "recovery_generation": status.get("generation_uid"),
            "recovery_manifest": status.get("manifest_id"),
        }

    def _wait_archie_owner(
        self,
        release: Release,
        predicate: Callable[[dict[str, Any]], bool],
        *,
        approved_spaces: set[str],
        require_quiet: bool,
        timeout: float,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        last: Exception | None = None
        while time.monotonic() < deadline:
            try:
                row = self._archie_owner_snapshot(
                    release, approved_spaces=approved_spaces, require_quiet=require_quiet
                )
                if predicate(row):
                    return row
            except RolloutError as error:
                last = error
            time.sleep(0.5)
        raise Refusal(f"Archie service did not reach the requested postcondition: {last}")

    def _remote_json(self, host: str, argv: Sequence[str]) -> Any:
        output = self.runner.capture(remote_argv(host, argv), timeout=40).stdout
        try:
            return json.loads(output)
        except json.JSONDecodeError as error:
            raise Refusal(f"remote command returned malformed JSON: {' '.join(argv)}") from error

    # Acceptance matrix ---------------------------------------------------

    def verify(self, release: Release) -> Release:
        for checkpoint in ("deploy.mac.service", "deploy.archie.service"):
            if not release.completed(checkpoint):
                raise Refusal(f"acceptance requires completed checkpoint {checkpoint}")
        self._ensure_primary_smoke(release)
        primary = release.data["smoke"]
        approved = {primary["space_uid"]}
        owner = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
        if primary["space_uid"] not in owner["spaces"]:
            raise Refusal("the journaled smoke Space is absent from the Mac owner")

        if not release.completed("verify.cold_present"):
            before = owner
            gui = self._present_and_wait(
                release,
                host=None,
                name=primary["name"],
                host_uid=primary["host_uid"],
                space_uid=primary["space_uid"],
            )
            time.sleep(2.0)
            stable = self._live_gui_for_space(primary["host_uid"], primary["space_uid"])
            after = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            self._require_same_owner_space(before, after, primary["space_uid"])
            self.store.checkpoint(
                release,
                "verify.cold_present",
                {"gui": stable, "owner": after, "initial_gui": gui},
            )

        if not release.completed("verify.reconnect"):
            before = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            first_gui = self._live_gui_for_space(primary["host_uid"], primary["space_uid"])
            second_gui = self._present_and_wait(
                release,
                host=None,
                name=primary["name"],
                host_uid=primary["host_uid"],
                space_uid=primary["space_uid"],
            )
            after = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            self._require_same_owner_space(before, after, primary["space_uid"])
            if (
                first_gui["pid"] != second_gui["pid"]
                or first_gui["pane_id"] != second_gui["pane_id"]
            ):
                raise Refusal(
                    "reconnect created a second GUI/pane rather than focusing the existing one"
                )
            self.store.checkpoint(
                release,
                "verify.reconnect",
                {"gui": second_gui, "owner": after},
            )

        if not release.completed("verify.lifecycle"):
            if not release.completed("verify.lifecycle.intent"):
                before = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
                gui = self._live_gui_for_space(primary["host_uid"], primary["space_uid"])
                self.store.checkpoint(
                    release, "verify.lifecycle.intent", {"owner": before, "gui": gui}
                )
            intent = release.checkpoints["verify.lifecycle.intent"]["evidence"]
            before = intent["owner"]
            gui = intent["gui"]
            try:
                live = self._live_gui_for_space(primary["host_uid"], primary["space_uid"])
                if live["gui_instance"] != gui["gui_instance"]:
                    raise Refusal("lifecycle GUI instance changed after intent was journaled")
                detached = self._safe_quit_gui(live)
            except Refusal:
                detached = self._require_gui_detached(gui)
            after = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            self._require_same_owner_space(before, after, primary["space_uid"])
            self.store.checkpoint(
                release,
                "verify.lifecycle",
                {"gui": detached, "owner": after},
            )

        self._verify_recovery_cycle(release, approved)
        self._verify_removal(release, approved)
        self._verify_two_host(release)
        release.set_phase("verified")
        self.store.save(release)
        return release

    def _ensure_primary_smoke(self, release: Release) -> None:
        smoke = release.data["smoke"]
        if smoke.get("space_uid") is None:
            receipt = self.runner.capture(
                [
                    str(self.config.mac_dmux),
                    "new",
                    smoke["name"],
                    "--backend",
                    "wez",
                    "--no-connect",
                ],
                env={"DMUX_WEZ_FIRST": "1"},
                unset_env=AMBIENT_MUX_VARS,
                timeout=60,
            ).stdout.strip()
            host_uid, space_uid = self._parse_new_receipt(receipt, backend="wez")
            release.set_smoke_identity(space_uid=space_uid, host_uid=host_uid)
            self.store.save(release)
            self.store.checkpoint(
                release,
                "verify.smoke_identity",
                {"host_uid": host_uid, "space_uid": space_uid, "receipt": receipt},
            )
        elif not release.completed("verify.smoke_identity"):
            self.store.checkpoint(
                release,
                "verify.smoke_identity",
                {
                    "host_uid": smoke["host_uid"],
                    "space_uid": smoke["space_uid"],
                    "adopted": True,
                },
            )

    @staticmethod
    def _parse_new_receipt(text: str, *, backend: str) -> tuple[str, str]:
        lines = [line for line in text.splitlines() if line.strip()]
        if len(lines) != 1:
            raise Refusal(f"dmux new returned {len(lines)} nonempty lines, expected one receipt")
        match = RECEIPT_RE.match(lines[0])
        if match is None or match.group("backend") != backend:
            raise Refusal(f"dmux new returned an invalid {backend} receipt")
        host_uid = require_space_uid(match.group("host"), "receipt host_uid")
        space_uid = require_space_uid(match.group("space"), "receipt space_uid")
        return host_uid, space_uid

    def _present_and_wait(
        self,
        release: Release,
        *,
        host: str | None,
        name: str,
        host_uid: str,
        space_uid: str,
    ) -> dict[str, Any]:
        argv = [
            str(self.config.mac_dmux),
            "con",
            "--name",
            name,
            "--backend",
            "wez",
            "--launch-gui",
        ]
        if host is not None:
            argv[2:2] = ["--host", host]
        self.runner.capture(
            argv,
            env={"DMUX_WEZ_FIRST": "1"},
            unset_env=AMBIENT_MUX_VARS,
            timeout=90,
        )
        deadline = time.monotonic() + 30
        last: Exception | None = None
        while time.monotonic() < deadline:
            try:
                return self._live_gui_for_space(host_uid, space_uid)
            except RolloutError as error:
                last = error
            time.sleep(0.1)
        raise Refusal(f"GUI never published the exact smoke marker: {last}")

    def _live_gui_for_space(self, host_uid: str, space_uid: str) -> dict[str, Any]:
        root = self._mac_runtime_dir() / "bridge/instances"
        found = []
        if not root.is_dir():
            raise Refusal("managed GUI heartbeat directory is absent")
        for heartbeat_path in root.glob("*/heartbeat.json"):
            try:
                heartbeat = self._load_bounded_json(heartbeat_path, maximum=1024 * 1024)
                self._require_live_heartbeat(heartbeat)
            except RolloutError:
                continue
            for pane in heartbeat.get("panes", []):
                context = pane.get("context", {}) if isinstance(pane, dict) else {}
                if context.get("host_uid") == host_uid and context.get("space_uid") == space_uid:
                    found.append(
                        {
                            "gui_instance": heartbeat["gui_instance"],
                            "pid": heartbeat["pid"],
                            "process_start_token": heartbeat["process_start_token"],
                            "heartbeat": str(heartbeat_path),
                            "updated_at": heartbeat["updated_at"],
                            "domain": pane.get("domain"),
                            "pane_id": pane.get("pane_id"),
                            "context": context,
                        }
                    )
        if len(found) != 1:
            raise Refusal(f"expected one live GUI pane for smoke Space, found {len(found)}")
        return found[0]

    def _require_live_heartbeat(self, heartbeat: Any) -> None:
        if not isinstance(heartbeat, dict):
            raise Refusal("heartbeat is not an object")
        pid = heartbeat.get("pid")
        token = heartbeat.get("process_start_token")
        updated = heartbeat.get("updated_at")
        if not isinstance(pid, int) or pid <= 0 or not isinstance(token, str):
            raise Refusal("heartbeat process identity is malformed")
        if not isinstance(updated, int) or abs(time.time() - updated) > 15:
            raise Refusal("heartbeat is stale")
        process = self.runner.capture(
            ["ps", "-p", str(pid), "-o", "lstart=", "-o", "comm="], check=False
        )
        if process.returncode != 0:
            raise Refusal("heartbeat process is gone")
        lines = process.stdout.strip().split()
        if "wezterm-gui" not in process.stdout or token not in process.stdout:
            raise Refusal("heartbeat PID/start token does not identify wezterm-gui")
        if not lines:
            raise Refusal("heartbeat process evidence is empty")

    def _safe_quit_gui(self, gui: dict[str, Any]) -> dict[str, Any]:
        pid = int(gui["pid"])
        script = (
            'tell application "System Events"\n'
            f"set frontmost of first process whose unix id is {pid} to true\n"
            'keystroke "q" using command down\n'
            "end tell"
        )
        self.runner.capture(["osascript", "-e", script], timeout=30)
        deadline = time.monotonic() + 30
        last: Any = None
        path = Path(gui["heartbeat"])
        while time.monotonic() < deadline:
            try:
                heartbeat = self._load_bounded_json(path, maximum=1024 * 1024)
                self._require_live_heartbeat(heartbeat)
                return self._detached_gui_state(gui, heartbeat)
            except RolloutError as error:
                last = str(error)
            time.sleep(0.1)
        raise Refusal(f"managed Cmd-Q did not reach exact detached/hidden postcondition: {last}")

    def _require_gui_detached(self, gui: dict[str, Any]) -> dict[str, Any]:
        heartbeat = self._load_bounded_json(Path(gui["heartbeat"]), maximum=1024 * 1024)
        self._require_live_heartbeat(heartbeat)
        return self._detached_gui_state(gui, heartbeat)

    @staticmethod
    def _detached_gui_state(gui: dict[str, Any], heartbeat: dict[str, Any]) -> dict[str, Any]:
        if heartbeat.get("gui_instance") != gui["gui_instance"]:
            raise Refusal("GUI heartbeat changed instance during lifecycle")
        panes = heartbeat.get("panes", [])
        target_present = any(
            pane.get("context", {}).get("space_uid") == gui["context"]["space_uid"]
            for pane in panes
            if isinstance(pane, dict)
        )
        domains = heartbeat.get("domains", {})
        detached = all(
            not isinstance(row, dict)
            or row.get("backend_instance_uid") is None
            or (
                row.get("state") == "Detached"
                and row.get("pane_count") == 0
                and row.get("system_pane_count") == 0
            )
            for row in domains.values()
        )
        if target_present or not detached:
            raise Refusal("GUI is not in the exact detached/hidden lifecycle state")
        return {
            "gui_instance": heartbeat["gui_instance"],
            "pid": heartbeat["pid"],
            "process_start_token": heartbeat["process_start_token"],
            "domains": domains,
            "panes": panes,
        }

    @staticmethod
    def _require_same_owner_space(
        before: dict[str, Any], after: dict[str, Any], space_uid: str
    ) -> None:
        for field in ("pid", "epoch", "backend_instance_uid", "socket_dev", "socket_ino"):
            if before[field] != after[field]:
                raise Refusal(f"owner identity changed during presentation/lifecycle: {field}")
        if before["spaces"].get(space_uid) != after["spaces"].get(space_uid):
            raise Refusal("owner pane IDs changed during presentation/lifecycle")

    def _verify_recovery_cycle(self, release: Release, approved: set[str]) -> None:
        if release.completed("verify.recovery"):
            current = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            frozen = release.checkpoints["verify.recovery"]["evidence"]["after"]
            if current["backend_instance_uid"] != frozen["backend_instance_uid"]:
                raise Refusal("recovered owner backend identity drifted")
            return
        if not release.completed("verify.recovery.intent"):
            before = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            self.store.checkpoint(release, "verify.recovery.intent", before)
        before = release.checkpoints["verify.recovery.intent"]["evidence"]
        current = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
        if current["pid"] != before["pid"] and current["epoch"] != before["epoch"]:
            after = current
        else:
            label = release.data["hosts"]["mac"]["service_label"]
            self.runner.capture(
                ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{label}"], timeout=60
            )
            after = self._wait_mac_owner(
                lambda row: row["pid"] != before["pid"] and row["epoch"] != before["epoch"],
                approved_spaces=approved,
                require_quiet=False,
                timeout=90,
            )
        if set(before["spaces"]) != set(after["spaces"]):
            raise Refusal("recovery did not restore the exact logical Space set")
        for uid, panes in before["spaces"].items():
            if len(panes) != len(after["spaces"].get(uid, [])):
                raise Refusal(f"recovery changed the pane count for Space {uid}")
        self.store.checkpoint(release, "verify.recovery", {"before": before, "after": after})

    def _verify_removal(self, release: Release, approved: set[str]) -> None:
        smoke = release.data["smoke"]
        removal = smoke.setdefault("removal", {"name": f"{smoke['name']}-remove"})
        if not release.completed("verify.removal_created"):
            receipt = self.runner.capture(
                [
                    str(self.config.mac_dmux),
                    "new",
                    removal["name"],
                    "--backend",
                    "wez",
                    "--no-connect",
                ],
                env={"DMUX_WEZ_FIRST": "1"},
                unset_env=AMBIENT_MUX_VARS,
                timeout=60,
            ).stdout.strip()
            host_uid, space_uid = self._parse_new_receipt(receipt, backend="wez")
            removal.update(
                {
                    "host_uid": host_uid,
                    "space_uid": space_uid,
                    "stable_ref": f"dmux://{host_uid}/spaces/{space_uid}",
                }
            )
            self.store.save(release)
            self.store.checkpoint(release, "verify.removal_created", dict(removal))
        removal_uid = removal["space_uid"]
        if release.completed("verify.removal"):
            snapshot = self._mac_owner_snapshot(approved_spaces=approved, require_quiet=False)
            if removal_uid in snapshot["spaces"]:
                raise Refusal("journal says removal completed but native pane remains")
            return
        allowed = approved | {removal_uid}
        before = self._mac_owner_snapshot(approved_spaces=allowed, require_quiet=False)
        if removal_uid not in before["spaces"]:
            self.store.checkpoint(
                release,
                "verify.removal",
                {"space_uid": removal_uid, "owner_epoch": before["epoch"], "resumed": True},
            )
            return
        self.runner.capture(
            [str(self.config.mac_dmux), "rm", removal["stable_ref"], "--yes"],
            env={"DMUX_WEZ_FIRST": "1"},
            unset_env=AMBIENT_MUX_VARS,
            timeout=60,
        )
        deadline = time.monotonic() + 30
        after = None
        while time.monotonic() < deadline:
            after = self._mac_owner_snapshot(approved_spaces=allowed, require_quiet=False)
            if removal_uid not in after["spaces"]:
                break
            time.sleep(0.1)
        if after is None or removal_uid in after["spaces"]:
            raise Refusal("removed Space remained in the owner inventory")
        self.store.checkpoint(
            release,
            "verify.removal",
            {"space_uid": removal_uid, "owner_epoch": after["epoch"]},
        )

    def _verify_two_host(self, release: Release) -> None:
        smoke = release.data["smoke"]
        remote = smoke.setdefault("remote", {"name": f"{smoke['name']}-archie"})
        host = release.data["hosts"]["archie"]["ssh"]
        if not release.completed("verify.two_host_identity"):
            receipt = self.runner.capture(
                [
                    str(self.config.mac_dmux),
                    "new",
                    "--host",
                    host,
                    remote["name"],
                    "--backend",
                    "wez",
                    "--no-connect",
                ],
                env={"DMUX_WEZ_FIRST": "1"},
                unset_env=AMBIENT_MUX_VARS,
                timeout=90,
            ).stdout.strip()
            host_uid, space_uid = self._parse_new_receipt(receipt, backend="wez")
            remote.update({"host_uid": host_uid, "space_uid": space_uid})
            self.store.save(release)
            self.store.checkpoint(release, "verify.two_host_identity", dict(remote))
        approved = {remote["space_uid"]}
        owner = self._archie_owner_snapshot(release, approved_spaces=approved, require_quiet=False)
        if remote["space_uid"] not in owner["spaces"]:
            raise Refusal("journaled Archie smoke Space is absent")
        if release.completed("verify.two_host"):
            return
        gui = self._present_and_wait(
            release,
            host=host,
            name=remote["name"],
            host_uid=remote["host_uid"],
            space_uid=remote["space_uid"],
        )
        stable_owner = self._archie_owner_snapshot(
            release, approved_spaces=approved, require_quiet=False
        )
        if stable_owner["spaces"].get(remote["space_uid"]) != owner["spaces"].get(
            remote["space_uid"]
        ):
            raise Refusal("two-host presentation mutated Archie owner panes")
        detached = self._safe_quit_gui(gui)
        final_owner = self._archie_owner_snapshot(
            release, approved_spaces=approved, require_quiet=False
        )
        if final_owner["spaces"] != stable_owner["spaces"]:
            raise Refusal("two-host lifecycle did not preserve Archie owner panes")
        self.store.checkpoint(
            release,
            "verify.two_host",
            {"gui": detached, "owner": final_owner},
        )

    # Rollback -------------------------------------------------------------

    def rollback(self, release: Release) -> str:
        if release.data["phase"] == "rolled_back":
            return "rolled_back"
        if release.completed("deploy.mac.install") and not release.completed("rollback.mac"):
            self._restore_mac_files(release, require_release_hash=True)
            self.runner.capture(
                ["codesign", "--force", "--deep", "--sign", "-", str(self.config.mac_app)],
                timeout=120,
            )
            self.runner.capture(
                ["codesign", "--verify", "--deep", "--strict", str(self.config.mac_app)]
            )
            previous = release.data["rollback"]["mac"].get("launchd_dmux_wez_first", "")
            if previous:
                self.runner.capture(["launchctl", "setenv", "DMUX_WEZ_FIRST", previous])
            else:
                self.runner.capture(["launchctl", "unsetenv", "DMUX_WEZ_FIRST"], check=False)
            label = release.data["hosts"]["mac"]["service_label"]
            self.runner.capture(
                ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{label}"], timeout=60
            )
            hashes = {
                Path(row["target"]).name: {
                    "path": row["target"],
                    "sha256": sha256_file(Path(row["target"])),
                }
                for row in release.data["rollback"]["mac"]["files"]
            }
            self.store.checkpoint(
                release,
                "rollback.mac",
                {"files": hashes, "durable_state_preserved": True},
            )

        if release.completed("deploy.archie.packages"):
            if not self._archie_rollback_packages_installed(release):
                command = self.archie_rollback_command(release)
                self.store.append_event(
                    release,
                    "awaiting_archie_rollback_pacman",
                    {"command": command, "durable_state_preserved": True},
                )
                return "awaiting_archie_rollback_pacman"
            if not release.completed("rollback.archie"):
                self._restore_archie_user_files(release)
                self._restore_archie_config(release)
                host = release.data["hosts"]["archie"]["ssh"]
                previous = release.data["rollback"]["archie"].get("dmux_wez_first", "")
                if previous:
                    self.runner.capture(
                        remote_argv(
                            host,
                            [
                                "systemctl",
                                "--user",
                                "set-environment",
                                f"DMUX_WEZ_FIRST={previous}",
                            ],
                        )
                    )
                else:
                    self.runner.capture(
                        remote_argv(
                            host, ["systemctl", "--user", "unset-environment", "DMUX_WEZ_FIRST"]
                        ),
                        check=False,
                    )
                self.runner.capture(remote_argv(host, ["systemctl", "--user", "daemon-reload"]))
                self.runner.capture(
                    remote_argv(
                        host, ["systemctl", "--user", "restart", "wezterm-mux-server.service"]
                    ),
                    timeout=60,
                )
                self.store.checkpoint(
                    release,
                    "rollback.archie",
                    {"durable_state_preserved": True},
                )
        release.set_phase("rolled_back")
        self.store.save(release)
        self.store.append_event(
            release,
            "rollback_complete",
            {
                "registry_deleted": False,
                "tombstones_deleted": False,
                "recovery_manifests_deleted": False,
            },
        )
        return "rolled_back"

    def archie_rollback_command(self, release: Release) -> str:
        import shlex

        rows = release.data["rollback"]["archie"].get("packages")
        if not isinstance(rows, dict) or set(rows) != {
            "wezterm-fredrir-git",
            "wezterm-fredrir-git-debug",
        }:
            raise StateError("Archie package rollback inventory is incomplete")
        paths = [rows["wezterm-fredrir-git"]["path"], rows["wezterm-fredrir-git-debug"]["path"]]
        inner = shlex.join(["sudo", "pacman", "-U", *paths])
        return shlex.join(["ssh", "-t", release.data["hosts"]["archie"]["ssh"], inner])

    def _archie_rollback_packages_installed(self, release: Release) -> bool:
        host = release.data["hosts"]["archie"]["ssh"]
        rows = release.data["rollback"]["archie"].get("packages", {})
        for name in ("wezterm-fredrir-git", "wezterm-fredrir-git-debug"):
            expected = rows.get(name, {}).get("version")
            if not expected:
                raise StateError("Archie rollback package version is absent")
            query = self.runner.capture(remote_argv(host, ["pacman", "-Q", name]), check=False)
            if query.returncode != 0 or query.stdout.strip() != f"{name} {expected}":
                return False
        return True

    def _restore_archie_user_files(self, release: Release) -> None:
        host = release.data["hosts"]["archie"]["ssh"]
        installed = release.data["artifacts"].get("archie_user_installed", {})
        rows = release.data["rollback"]["archie"].get("user_files")
        if not isinstance(rows, list) or len(rows) != 2:
            raise StateError("Archie user binary rollback inventory is incomplete")
        for row in rows:
            target = Path(row["target"])
            name = target.name
            expected = installed.get(name, {}).get("sha256")
            current = self._remote_artifacts(host, {name: target})[name]["sha256"]
            if current != expected:
                raise Refusal(f"Archie rollback target was modified after deployment: {target}")
            backup = Path(row["backup"])
            backup_hash = self._remote_artifacts(host, {name: backup})[name]["sha256"]
            if backup_hash != row["before_sha256"]:
                raise Refusal(f"Archie rollback binary changed: {backup}")
            temporary = target.parent / f".{name}.{release.release_id}.rollback.tmp"
            self.runner.capture(
                remote_argv(host, ["install", "-m", "0755", "--", str(backup), str(temporary)])
            )
            self.runner.capture(remote_argv(host, ["mv", "-fT", "--", str(temporary), str(target)]))

    def _restore_archie_config(self, release: Release) -> None:
        if not release.completed("deploy.archie.config"):
            return
        host = release.data["hosts"]["archie"]["ssh"]
        config = release.data["rollback"]["archie"].get("config")
        if not isinstance(config, dict):
            raise StateError("Archie config rollback witness is absent")
        repo = Path(config["repo"])
        old = config["head"]
        deployed = config["release_head"]
        if old == deployed:
            return
        current = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "rev-parse", "HEAD"])
        ).stdout.strip()
        changed = config["changed"]
        paths = [row["path"] for row in changed]
        if current == deployed:
            cached = self.runner.capture(
                remote_argv(host, ["git", "-C", str(repo), "diff", "--cached", "--quiet"]),
                check=False,
            )
            if cached.returncode != 0:
                raise Refusal("Archie config gained staged changes after deployment")
            for path in paths:
                clean = self.runner.capture(
                    remote_argv(
                        host,
                        ["git", "-C", str(repo), "diff", "--quiet", deployed, "--", path],
                    ),
                    check=False,
                )
                if clean.returncode != 0:
                    raise Refusal(f"Archie release-managed config changed after deployment: {path}")
            self.runner.capture(
                remote_argv(
                    host,
                    [
                        "git",
                        "-C",
                        str(repo),
                        "update-ref",
                        f"refs/heads/{config['branch']}",
                        old,
                        deployed,
                    ],
                )
            )
            self.runner.capture(remote_argv(host, ["git", "-C", str(repo), "reset", "--mixed"]))
        elif current != old:
            raise Refusal(f"Archie config moved to an unjournaled commit: {current}")
        restore = [row["path"] for row in changed if row["status"] in {"M", "D"}]
        if restore:
            self.runner.capture(
                remote_argv(host, ["git", "-C", str(repo), "restore", "--worktree", "--", *restore])
            )
        for path in (row["path"] for row in changed if row["status"] == "A"):
            target = repo / path
            tracked = self.runner.capture(
                remote_argv(host, ["git", "-C", str(repo), "ls-files", "--error-unmatch", path]),
                check=False,
            )
            if tracked.returncode == 0:
                raise Refusal(f"rollback-added path unexpectedly remains tracked: {path}")
            self.runner.capture(remote_argv(host, ["rm", "--", str(target)]))
        actual = self.runner.capture(
            remote_argv(host, ["git", "-C", str(repo), "rev-parse", "HEAD"])
        ).stdout.strip()
        dirty = self._remote_git_dirty(host, repo)
        if actual != old or dirty != config["dirty"]:
            raise Refusal("Archie config rollback did not restore its exact prior state")
