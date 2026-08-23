from __future__ import annotations

import json
import os
from collections.abc import Callable
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

import typer

from tools.core.paths import dotfiles_root
from tools.dmux_rollout.command import Runner
from tools.dmux_rollout.errors import RolloutError, StateError
from tools.dmux_rollout.model import Release
from tools.dmux_rollout.storage import RolloutStore, default_state_root
from tools.dmux_rollout.workflow import Workflow, WorkflowConfig

app = typer.Typer(
    add_completion=False,
    no_args_is_help=True,
    help="Build, deploy, verify, resume, and roll back one exact dmux release.",
)


@dataclass
class Context:
    store: RolloutStore
    workflow: Workflow
    release_id: str | None


def _state_root() -> Path:
    testing = os.environ.get("DMUX_ROLLOUT_TEST_STATE_ROOT")
    if testing:
        if os.environ.get("DMUX_ROLLOUT_TESTING") != "1":
            raise StateError(
                "DMUX_ROLLOUT_TEST_STATE_ROOT is accepted only with DMUX_ROLLOUT_TESTING=1"
            )
        return Path(testing).absolute()
    return default_state_root()


@app.callback()
def main(
    ctx: typer.Context,
    release: str | None = typer.Option(
        None,
        "--release",
        help="Release ID; defaults to the private journal's active release.",
    ),
):
    try:
        store = RolloutStore(_state_root())
        workflow = Workflow(store, Runner(), WorkflowConfig.production(dotfiles_root()))
        ctx.obj = Context(store, workflow, release)
    except RolloutError as error:
        _die(error)


def _die(error: Exception, *, code: int = 1) -> None:
    typer.echo(f"dmux-rollout: {error}", err=True)
    raise typer.Exit(code)


def _locked[T](context: Context, action: Callable[[], T]) -> T:
    try:
        with context.store.exclusive():
            return action()
    except RolloutError as error:
        _die(error)
    raise AssertionError("unreachable")


def _load(context: Context) -> Release:
    return context.store.load(context.release_id)


def _summary(release: Release) -> None:
    frozen = release.data["frozen"]
    typer.echo(f"release:   {release.release_id}")
    typer.echo(f"phase:     {release.data['phase']}")
    typer.echo(f"dotfiles:  {frozen['dotfiles']['commit']}")
    typer.echo(f"wezterm:   {frozen['wezterm']['commit']}")
    typer.echo(f"archie:    {release.data['hosts']['archie']['ssh']}")
    typer.echo(f"smoke:     {release.data['smoke']['name']}")
    typer.echo(f"SpaceUid:  {release.data['smoke'].get('space_uid') or '-'}")
    typer.echo(f"checkpoints: {len(release.checkpoints)}")


@app.command(help="Freeze exact pushed source commits and create/reuse one release journal.")
def plan(
    ctx: typer.Context,
    dotfiles_ref: str = typer.Option("HEAD", "--dotfiles-ref"),
    wezterm_ref: str = typer.Option("HEAD", "--wezterm-ref"),
    release_id: str | None = typer.Option(None, "--release-id"),
    archie_ssh: str | None = typer.Option(
        None,
        "--archie-ssh",
        help=(
            "SSH destination for Archie (user@host), frozen into the manifest for every later "
            "ssh/scp/`dmux --host` call. Name an enrolled route, e.g. fredrir@10.77.77.2 (usb); "
            "the bare 'archie' alias is a disabled route. Default: archie."
        ),
    ),
    smoke_name: str | None = typer.Option(None, "--smoke-name"),
    smoke_space_uid: str | None = typer.Option(None, "--smoke-space-uid"),
    smoke_host_uid: str | None = typer.Option(None, "--smoke-host-uid"),
):
    context: Context = ctx.obj
    name = smoke_name or f"dmux-rollout-{datetime.now(UTC):%Y%m%d}"
    release = _locked(
        context,
        lambda: context.workflow.plan(
            dotfiles_ref=dotfiles_ref,
            wezterm_ref=wezterm_ref,
            release_id=release_id or context.release_id,
            smoke_name=name,
            smoke_space_uid=smoke_space_uid,
            smoke_host_uid=smoke_host_uid,
            archie_ssh=archie_ssh,
        ),
    )
    _summary(release)
    typer.echo(f"manifest:  {context.store.manifest_path(release.release_id)}")


@app.command(help="Build and test Mac dmux and WezTerm artifacts in clean detached worktrees.")
def build(ctx: typer.Context):
    context: Context = ctx.obj
    release = _locked(context, lambda: context.workflow.build(_load(context)))
    _summary(release)


@app.command(
    "deploy-mac", help="Atomically install/sign Mac artifacts and restart the exact service."
)
def deploy_mac(
    ctx: typer.Context,
    approve_space: list[str] = typer.Option(
        [],
        "--approve-space",
        help="Explicitly permit this pre-existing SpaceUid; repeatable.",
    ),
):
    context: Context = ctx.obj
    release = _locked(
        context,
        lambda: context.workflow.deploy_mac(_load(context), approved_spaces=set(approve_space)),
    )
    _summary(release)


@app.command(
    "stage-archie", help="Build exact Archie binaries/packages, then print the sudo pause."
)
def stage_archie(ctx: typer.Context):
    context: Context = ctx.obj
    release = _locked(context, lambda: context.workflow.stage_archie(_load(context)))
    _summary(release)
    typer.echo("\nInteractive Archie package step (the runner will not enter sudo):")
    typer.echo(context.workflow.archie_install_command(release))


@app.command(help="Detect the staged pacman install, then continue Archie deployment exactly once.")
def resume(
    ctx: typer.Context,
    approve_space: list[str] = typer.Option([], "--approve-space"),
):
    context: Context = ctx.obj

    def action() -> tuple[Release, str]:
        release = _load(context)
        return release, context.workflow.resume(release, approved_spaces=set(approve_space))

    release, result = _locked(context, action)
    if result == "awaiting_archie_pacman":
        typer.echo("Archie is still on the prior packages. Run exactly:")
        typer.echo(context.workflow.archie_install_command(release))
        raise typer.Exit(4)
    _summary(release)


@app.command(help="Run the journaled cold/reconnect/lifecycle/recovery/removal/two-host matrix.")
def verify(
    ctx: typer.Context,
    approve_space: list[str] = typer.Option([], "--approve-space"),
):
    context: Context = ctx.obj
    release = _locked(
        context,
        lambda: context.workflow.verify(_load(context), approved_spaces=set(approve_space)),
    )
    _summary(release)


@app.command(help="Restore recorded binaries/packages while preserving registry and user state.")
def rollback(ctx: typer.Context):
    context: Context = ctx.obj

    def action() -> tuple[Release, str]:
        release = _load(context)
        return release, context.workflow.rollback(release)

    release, result = _locked(context, action)
    if result == "awaiting_archie_rollback_pacman":
        typer.echo("Mac rollback is complete. Restore Archie's exact packages with:")
        typer.echo(context.workflow.archie_rollback_command(release))
        typer.echo("Then rerun dmux-rollout rollback; registry/tombstones were not touched.")
        raise typer.Exit(4)
    _summary(release)


@app.command(help="Show the active release and completed checkpoints.")
def status(
    ctx: typer.Context,
    as_json: bool = typer.Option(False, "--json", help="Print the complete release manifest."),
):
    context: Context = ctx.obj
    release = _locked(context, lambda: _load(context))
    if as_json:
        typer.echo(json.dumps(release.data, indent=2, sort_keys=True))
    else:
        _summary(release)
        for name, row in sorted(release.checkpoints.items()):
            typer.echo(f"  {row['at']}  {name}")


if __name__ == "__main__":
    app()
