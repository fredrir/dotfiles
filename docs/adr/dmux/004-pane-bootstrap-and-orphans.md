# ADR 004: Provisional pane bootstrap and orphan recovery (P0 spike 4)

Status: accepted (P0 evidence; mechanism selected)
Date: 2026-08-16
Evidence: repo `docs/adr/dmux/evidence/spike4-pane-bootstrap.md` (wezterm 20260813-114614-18a44cb7, tmux 3.7b)
Plan refs: §11.1 bootstrap, §11.2, §15.3 step 7

## Decision

The provisional bootstrap helper + FIFO handshake is feasible and selected on
both providers. Correlation is three-way and exact on every creation path:
spawn-return ID = reserved-title scan (`dmux-bootstrap:<request-uid>`, count
must be exactly 1) = helper-recorded inherited `WEZTERM_PANE`/`TMUX_PANE`.

## Frozen spawn-return formats

| Verb | stdout |
| --- | --- |
| `wezterm cli spawn --new-window --workspace <key> -- <argv>` | `<pane_id>\n` only |
| `wezterm cli spawn --window-id <id> -- <argv>` | `<pane_id>\n` only |
| `wezterm cli split-pane --pane-id <id> -- <argv>` | `<pane_id>\n` only |
| `tmux new-session -d -P -F '#{session_id}|#{window_id}|#{pane_id}'` | `$N|@N|%N` |
| `tmux new-window -P -F '...'` | `$N|@N|%N` |
| `tmux split-window -P -F '...'` | `$N|@N|%N` |

Wez emits no window/tab id and has no format flag; window/tab association
comes only from post-spawn `list --format json` correlation. tmux returns all
three IDs atomically at creation.

## Frozen handshake shape

1. Coordinator records the `bootstrap_request` journal row, pre-creates the
   per-uid FIFO under the runtime dir, then spawns `helper <uid>` as the pane
   program.
2. Helper immediately: sets reserved title via OSC 2, records inherited pane
   env to a per-uid file, opens the FIFO **O_RDWR** and does a bounded read.
   (Critical: a plain read-only open blocks in `open(2)` and voids the read
   timeout — the Rust helper must open RDWR or nonblocking.)
3. Coordinator correlates three ways against a complete same-epoch scan, then
   writes the payload (refs, env, argv).
4. Helper emits the final marker (SetUserVar + title), writes its ack, then
   `exec`s the payload argv **in place** — pane ID and PID are preserved
   (proven: same PID, parent = mux server; same `%N` on tmux).
5. Timeout path: helper writes a visible marker file, exits with code 41, and
   never runs user code.

## Orphan-recovery matrix (proven identically on both providers)

| Case | Result |
| --- | --- |
| Coordinator crash, helper blocked | title scan finds exactly one orphan |
| Helper timeout | marker file + exit 41; **pane closes — no linger** (wez single-pane workspace vanishes too; tmux `remain-on-exit off`) |
| Proven live orphan | killed by exact pane id; re-list confirms absence |
| Duplicate uid (two panes, same reserved title) | multiplicity=2 detected → conflict, no kill |
| Lost spawn-return (stdout discarded) | before/after pane-id-set diff + title scan still identify uniquely |

Because a timed-out pane self-closes, takeover may find **zero** orphans for a
journaled request; the plan's "retry only after confirmed absence" rule is
load-bearing, not defensive.

## tmux specifics

- OSC 2 title landing is gated by `allow-set-title on` (3.7b default);
  `allow-rename` is irrelevant (window name only). Managed servers should
  assert `allow-set-title` where title correlation is relied on.
- The user's tmux config leaks into managed servers (`base-index 1` broke
  `-t session:0` targeting in the spike). **ID-only targeting (`%N`, `@N`,
  `$N`) is mandatory**, confirming the plan's rule.
- Marker stamping proven: `@dmux_space_uid` set/read on a session survives
  external `rename-session` and stays readable via immutable `$N`; global
  `@dmux_server_epoch` set/read works.

## Hazards discovered for other phases

- **`WEZTERM_UNIX_SOCKET` does not control a server's listen path** — only
  `unix_domains.socket_path` in the config does. Started env-only, the spike
  server bound the default `~/.local/share/wezterm/sock` (caught and killed;
  no user mux-server existed there). Additionally `--daemonize` contends the
  shared default `~/.local/share/wezterm/pid` lock. Consequence for P5/ADR
  002: the service manager must generate a config with an explicit
  `socket_path` and run the server **foreground** under launchd/systemd.
- `wezterm cli list` exposes no user-vars and no foreground-process fields:
  SetUserVar emission is proven mechanically, but marker verification must not
  be designed around CLI readback (GUI-side observation or other channels).
- Fresh mux server auto-spawns one default shell pane (see ADR 001); bootstrap
  inventory logic must expect it until suppression (ADR 002) is active.
