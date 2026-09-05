# agent-hop

## Commands

<!-- cli:commands:start -->
| Command             | Description                                                                   |
| ------------------- | ----------------------------------------------------------------------------- |
| `agent-hop`         | Moves a Codex or Claude Code CLI session between the two workstations.        |
| `agent-hop run`     | Starts a managed native agent session that can transfer execution.            |
| `agent-hop move`    | Queues a managed agent handoff after its active work reaches a safe boundary. |
| `agent-hop status`  | Shows the recorded ownership and handoff state of a managed run.              |
| `agent-hop cancel`  | Cancels a queued move before ownership transfers.                             |
| `agent-hop follow`  | Attaches to the destination that owns the moved agent.                        |
| `agent-hop recover` | Resolves destination ownership before recovering preserved source history.    |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                               |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| `-n`, `--dry-run`       | Reports what would be copied and started without changing either machine.                 |
| `--no-connect`          | Copies the session without opening it on the other workstation.                           |
| `--color <WHEN>`        | Chooses `auto`, `always`, or `never` color output.                                        |
| `--list`                | Prints the local and remote sessions as tab-separated rows instead of opening the picker. |
| `--resume <RESUME>`     | Starts a managed run from this agent conversation ID.                                     |
| `--pane <PANE>`         | Selects a managed tmux pane; defaults to the current pane.                                |
| `--to <TO>`             | Selects `archie` or `macie` as the execution destination.                                 |
| `--run <RUN>`           | Selects a durable managed-run ID instead of a tmux pane.                                  |
| `-h`, `--help`          | Shows help for the selected command and exits.                                            |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                           |
| `-V`, `--version`       | Prints the version and exits.                                                             |
<!-- cli:flags:end -->

## Arguments

| Argument     | Value                                                              |
| ------------ | ------------------------------------------------------------------ |
| `AGENT`      | `codex` or `claude`                                                |
| `SESSION_ID` | Requires `AGENT`; the newest session for this directory without it |

## Invocation

| Command                          | Behavior                                          |
| -------------------------------- | ------------------------------------------------- |
| `agent-hop`                      | Opens the picker over local and remote sessions   |
| `agent-hop <AGENT>`              | Sends this directory's newest session to the peer |
| `agent-hop <AGENT> <SESSION_ID>` | Sends that session                                |
| `agent-hop --list`               | Prints the same catalog as tab-separated rows     |

`--list` conflicts with `AGENT`, `SESSION_ID`, `--dry-run`, and `--no-connect`.
Its columns are `HOST`, `ORIGIN`, `AGENT`, `UPDATED`, `WORKSPACE`, `TITLE`, and
`SESSION`; warnings go to standard error.

Without a TTY on both standard input and output, or with `TERM=dumb`, a bare
`agent-hop` prints its help instead of opening the picker.

## Transfer

These legacy picker/positional transfers copy conversation history. They do not
stop an already-running source agent. For execution takeover, use a managed run.

| Step      | Value                                                                  |
| --------- | ---------------------------------------------------------------------- |
| Copied    | The session's whole transcript lineage, plus its companion attachments |
| Reused    | Destination objects whose contents already match are not resent        |
| Refused   | A destination that exists with different contents                      |
| Recorded  | A transfer manifest on both machines                                   |
| Then      | The agent is launched on the peer, unless `--no-connect`               |

A previous hop that produced a child session on the peer is offered for resume
before a new copy is made.

## Managed execution

```sh
agent-hop run codex                 # inside a tmux pane
agent-hop run claude
agent-hop run codex --resume THREAD_ID
agent-hop move --pane %12 --to macie # may be requested while the agent is busy
agent-hop status --pane %12
agent-hop follow --pane %12
agent-hop cancel --pane %12         # cancel a queued move
agent-hop recover --pane %12        # resolve ownership before recovering
agent-hop status --run RUN_ID       # receipt survives pane closure
```

The [tmux action palette](../tmux.md) exposes managed launch, status and follow;
Ctrl-b A queues execution handoff for the selected pane.

| Operation | Contract |
| --- | --- |
| Queued move | Waits for the current turn and supported active work to finish |
| Preparation | Copies validated conversation lineage and a Git workspace snapshot |
| Destination | Fresh transaction-specific checkout; destination-local authentication and configuration |
| Ownership | Source managed turn execution must stop before destination turns are enabled |
| Successful move | Destination runs in a durable tmux session; no source-side tunnel is needed |
| Failed preparation | Source remains authoritative; inspect the recorded error |
| Uncertain commit | Do not launch a second copy; recover checks destination ownership first |

Codex preserves project trust for the validated private checkout through scoped
per-process configuration, including recovery only within that original checkout.
This allows the moved project's configuration, hooks and exec policies to load.
It does not edit global trust or pass sandbox/approval bypass flags; destination
machine policy constraints remain in force.
Configured startup hooks and MCP initialization may run during preparation;
the ownership fence controls conversational/model/tool-start execution, not
every initialization command.

Managed root goals pause while the current turn drains, then continue only after
destination ownership commits. Remaining token budgets carry over; historical
usage/time stay in the receipt because Codex cannot import cumulative counters.
An exhausted budget refuses transfer instead of granting additional tokens.

Only a successful `moved` receipt confirms the execution transfer. Check status
before shutting down the source. This covers the managed agent, not unrelated
programs elsewhere on the source machine.

Snapshots preserve committed history, staged and unstaged changes, and untracked
non-ignored files. They do not copy agent credential stores, ignored dependency trees,
arbitrary process memory, open network connections or unsupported submodules.
Tracked and non-ignored project files transfer as-is. Do not track secrets;
ignore untracked secret files to keep them out of the snapshot.
External services and daemonized helpers are not migrated, even if the agent
started them. Destination tools, services and project dependencies must be
available locally. Independent active child-agent goals are refused; they cannot
be silently discarded during a root-session move.

An unmanaged active agent cannot be retroactively given safe execution ownership:
finish or stop it deliberately, then resume its conversation through `run`.
Linux-to-macOS handoff resumes the conversation and workspace in a new native
process; it does not checkpoint an arbitrary live process across operating systems.
An empty new Codex session cannot move until its first conversation has produced
a persisted transcript.
