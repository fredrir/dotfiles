# hwire

## Commands

<!-- cli:commands:start -->
| Command       | Description                                                                   |
| ------------- | ----------------------------------------------------------------------------- |
| `hwire`       | Inspects connections or measures latency and throughput between two machines. |
| `hwire serve` | Answers measurements until told to stop.                                      |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                     | Description                                                       |
| ------------------------ | ----------------------------------------------------------------- |
| `-r`, `--route <ROUTE>`  | Selects the cable, Wi-Fi, LAN, or Tailscale route to measure.     |
| `-a`, `--all`            | Measures every available route sequentially.                      |
| `-b`, `--both`           | Provides a compatibility spelling for `--all`.                    |
| `-t`, `--time <SECONDS>` | Sets the transfer duration for each direction.                    |
| `-P`, `--streams <N>`    | Sets the number of concurrent transfer connections.               |
| `-n`, `--samples <N>`    | Limits the number of round trips timed.                           |
| `-l`, `--latency`        | Measures round-trip latency without running transfers.            |
| `-u`, `--up`             | Transfers only from this machine to the peer.                     |
| `-d`, `--down`           | Transfers only from the peer to this machine.                     |
| `--at <ADDRESS:PORT>`    | Measures an already-running server without starting one over SSH. |
| `--token <HEX>`          | Uses or requires the server's authentication token.               |
| `--json`                 | Prints connection information or measurements as JSON.            |
| `-i`, `--info`           | Shows the current connection or the routes available to the peer. |
| `-v`, `--verbose`        | Shows detailed information, with an animated view on a terminal.  |
| `--watch`                | Refreshes connection information as routes change.                |
| `--interval <SECONDS>`   | Sets the refresh interval for watched connection information.     |
| `--notify`               | Rings the terminal bell when the preferred route changes.         |
| `--color <WHEN>`         | Chooses `auto`, `always`, or `never` color output.                |
| `--bind <ADDRESS>`       | Sets the address on which `hwire serve` listens.                  |
| `--port <PORT>`          | Sets the server port, with zero selecting an available port.      |
| `--idle <SECONDS>`       | Sets how long an idle server waits before exiting.                |
| `-h`, `--help`           | Shows help for the selected command and exits.                    |
| `--completions <SHELL>`  | Prints a shell completion script for the named shell and exits.   |
| `-V`, `--version`        | Prints the version and exits.                                     |
<!-- cli:flags:end -->

## Connection information

`hwire -i` replaces the former `hpath` shell function. There is no `hpath`
command or compatibility alias: use `hwire -i` for connection status and
`hwire -i HOST...` to inspect one or more SSH targets.

With no host argument, compact mode describes either the current remote
session or the routes a new connection to the peer could use:

```console
$ hwire -i
TAILSCALE | LAN

$ hwire -i
LAN                                                             archie --> macie

$ hwire -i
CABLE - TLS                                                      macie --> archie
```

A local result contains every route that answered the concurrent probes. The
order is the reverse of connection preference, so the route a new connection
would select is last and is printed in bold color. Connection preference is
`CABLE`, `WIFI`, `LAN`, then `TAILSCALE`. A remote result contains only the
route carrying that session and adds `TLS` for a WezTerm TLS connection. Its
destination host is bold and colored.

On a terminal, the remote endpoints are right-aligned across half the terminal
width, capped at 80 columns. Redirected output has one separating space and no
alignment padding.
`--color auto` is the default and produces no ANSI escapes when redirected;
`always` deliberately forces color and `never` disables it. Automatic color
also honors `NO_COLOR`, `CLICOLOR=0`, and a dumb terminal.

If no route answers, compact mode prints `UNREACHABLE`. If session evidence is
present but its address cannot be classified safely, it prints `UNKNOWN`
instead of guessing that the route is LAN.

### Explicit SSH targets

Each argument to `hwire -i HOST...` is resolved independently with the normal
OpenSSH configuration. Compact mode prints one route and endpoint line per
target. For example:

```console
$ hwire -i archie lan-archie 100.126.231.24
CABLE macie --> archie
LAN macie --> lan-archie
TAILSCALE macie --> 100.126.231.24
```

Use verbose mode when the chosen route alone is not enough:

```console
$ hwire -iv archie
```

The Ratatui view shows the resolved hostname, user and port, bind address or
interface, proxy command, route addresses and probe timings. For explicit SSH
targets it also checks the ControlMaster, reports whether it is running, and
shows its socket path, socket age, and OpenSSH diagnostic. TachyonFX animates
real state transitions without delaying the initial result. Long diagnostics
can be scrolled with the arrow, `j`/`k`, and page keys; `q`, Escape, or
Control-C closes a watched view.

### Watching routes

`--watch` keeps the information view current. `--interval SECONDS` selects its
refresh interval, and `--notify` emits a terminal bell whenever the preferred
route changes:

```console
$ hwire -iv --watch --interval 0.5 --notify
```

`--verbose`, `--watch`, `--interval`, and `--notify` are information options
and therefore require `--info`. Watching does not start a daemon or write a
cache; each refresh is a fresh, concurrent snapshot.

### JSON

`hwire -i --json` returns the same snapshot as a stable JSON document, without
the TUI, color, animation, or terminal padding. It includes the mode, local and
peer hosts, preferred and available routes, session evidence, per-route
addresses and elapsed probe time, resolved targets, ControlMaster diagnostics,
and warnings. Explicit hosts work with JSON too:

```console
$ hwire -i --json archie lan-archie
```

The document carries `schema_version: 1`. With `--watch`, each meaningful
state change is emitted as one complete JSON object on its own line (NDJSON).

## Session detection

SSH sessions are classified from `SSH_CONNECTION`, including the actual local
address accepted by the server; changing the preferred route cannot rewrite an
existing session. WezTerm TLS panes carry a validated `HWIRE_SESSION` stamp
containing the origin, destination, and selected route. `WEZTERM_HOSTNAME`
alone cannot prove that a pane is remote or say which mux route it uses.

TLS panes opened before this integration have no validated stamp. Reopen those
legacy panes once so `hwire -i` can report `TLS` and its real route. New tabs
and splits opened from a stamped remote pane inherit the metadata.

## Completion

The zsh completion is generated from the Rust parser with:

```console
$ hwire --completions zsh
```

The repository's completion loader installs this automatically. The zsh layer
is mode-aware: information-only flags and SSH host completion appear after
`--info`, while interval and notification flags appear after `--watch`. Its
descriptions and value names come from the clap definitions, so they match
`--help`.

## Performance gates

The information path has release-mode median latency gates: 3 ms for an
already-established remote session and 75 ms for a healthy local route
snapshot. Run them on either workstation with:

```console
$ cd scripts/rust
$ cargo build --release -p hwire
$ cargo bench -p hwire --bench info
```
