# agent-hop

## Commands

<!-- cli:commands:start -->
| Command     | Description                                                            |
| ----------- | ---------------------------------------------------------------------- |
| `agent-hop` | Moves a Codex or Claude Code CLI session between the two workstations. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                               |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| `-n`, `--dry-run`       | Reports what would be copied and started without changing either machine.                 |
| `--no-connect`          | Copies the session without opening it on the other workstation.                           |
| `--color <WHEN>`        | Chooses `auto`, `always`, or `never` color output.                                        |
| `--list`                | Prints the local and remote sessions as tab-separated rows instead of opening the picker. |
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

| Step      | Value                                                                  |
| --------- | ---------------------------------------------------------------------- |
| Copied    | The session's whole transcript lineage, plus its companion attachments |
| Reused    | Destination objects whose contents already match are not resent        |
| Refused   | A destination that exists with different contents                      |
| Recorded  | A transfer manifest on both machines                                   |
| Then      | The agent is launched on the peer, unless `--no-connect`               |

A previous hop that produced a child session on the peer is offered for resume
before a new copy is made.
