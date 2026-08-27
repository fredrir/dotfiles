# hwire

## Commands

<!-- cli:commands:start -->
| Command       | Description                                           |
| ------------- | ----------------------------------------------------- |
| `hwire`       | Measures latency and throughput between two machines. |
| `hwire serve` | Answers measurements until told to stop.              |
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
| `--json`                 | Prints the measurement as JSON.                                   |
| `--bind <ADDRESS>`       | Sets the address on which `hwire serve` listens.                  |
| `--port <PORT>`          | Sets the server port, with zero selecting an available port.      |
| `--idle <SECONDS>`       | Sets how long an idle server waits before exiting.                |
| `-h`, `--help`           | Shows help for the selected command and exits.                    |
| `--completions <SHELL>`  | Prints a shell completion script for the named shell and exits.   |
| `-V`, `--version`        | Prints the version and exits.                                     |
<!-- cli:flags:end -->
