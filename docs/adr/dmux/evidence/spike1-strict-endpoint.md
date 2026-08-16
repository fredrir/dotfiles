# Spike 1 — Strict exact-socket endpoint selection (plan §8.1 / §11.1)

Date: 2026-08-16. Host: macOS (Darwin 25.5.0). wezterm 20260813-114614-18a44cb7 (fredrir fork build) at
/opt/homebrew/bin/wezterm, /opt/homebrew/bin/wezterm-mux-server. Live user GUI (pid 9640) never touched;
every `wezterm cli` invocation used an explicit scratch `WEZTERM_UNIX_SOCKET` + `--no-auto-start`.

## Verdict

**Stock CLI + dmux-side verification suffices. No forked strict selector is required.**

`WEZTERM_UNIX_SOCKET=<exact-socket>` makes stock `wezterm cli --no-auto-start` connect to exactly that
socket — config order is irrelevant, the config does not even need to contain a matching `unix_domains`
entry, and `--prefer-mux` is unnecessary. What stock CLI does NOT give is typed failure classification
(stderr is generic for 3 of 4 failure modes), a connect/handshake timeout (it hangs forever on a silent
listener), or any server identity field. All three gaps are closed dmux-side with a cheap pre-flight
probe (lstat/stat + connect(2) errno + `LOCAL_PEERPID`) plus the sentinel-workspace check on the same
`list` JSON the operation consumes, plus a mandatory dmux-imposed timeout on every CLI child.

### Frozen invocation template

```
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE \
  WEZTERM_UNIX_SOCKET=<exact-service-socket> \
  wezterm --config-file <dmux-managed-config> cli --no-auto-start <subcommand> ... [--format json]
```

(Rust builds argv/env directly; `--config-file` content is irrelevant to endpoint selection but should
stay pinned to a dmux-owned file so user config can never inject Lua side effects. Every child gets a
dmux-side deadline; wezterm has none.)

## Environment / scratch layout

Scratchpad path is >104 bytes for sun_path, so a short symlink dir was used:
`/tmp/dmux-s1 -> /private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike1`.
Sockets were bound via the short path (`/tmp/dmux-s1/a.sock` = 20 bytes). **Implication for dmux: the
service must publish short socket paths (e.g. under `~/.dmux/run/` equivalent kept short, or `/tmp`),
respecting the ~104-byte macOS `sun_path` limit.**

## Task 1 — Two isolated mux servers

Config shape (per instance; this alone makes `wezterm-mux-server` serve that socket):

```lua
-- /tmp/dmux-s1/a.lua
return {
  unix_domains = { { name = "spike-a", socket_path = "/tmp/dmux-s1/a.sock" } },
  default_workspace = "A-default",
  default_prog = { "/bin/sleep", "3600" },
}
-- b.lua identical with spike-b / b.sock / B-default
```

Started (no daemonize, logs to file):

```
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE -u WEZTERM_UNIX_SOCKET \
  nohup /opt/homebrew/bin/wezterm-mux-server --config-file /tmp/dmux-s1/a.lua &   # pid 34842
  ... --config-file /tmp/dmux-s1/b.lua &                                          # pid 34843
```

`lsof -p <pid> -a -U` confirmed each pid bound ONLY its scratch socket (A: `/tmp/dmux-s1/a.sock`,
B: `/tmp/dmux-s1/b.sock`) plus one internal socketpair. No default-runtime-dir socket was created.
Note: **wezterm-mux-server spawns one default pane at startup** (pane 0, workspace = `default_workspace`,
running `default_prog`) — a "fresh" server's inventory is never empty.

Distinguishing workspaces spawned via exact socket (`cli --no-auto-start spawn --new-window
--workspace SPIKE-WS-A -- /bin/sleep 3600`, exit 0, printed new pane id `1`); same for B.

## Task 2 — Exact-socket selector beats config order (both directions)

Single client config `/tmp/dmux-s1/ab.lua` listing BOTH domains, **A first**:

```lua
return { unix_domains = {
  { name = "spike-a", socket_path = "/tmp/dmux-s1/a.sock" },
  { name = "spike-b", socket_path = "/tmp/dmux-s1/b.sock" },
} }
```

| argv/env (all with `env -u WEZTERM_PANE -u TMUX -u TMUX_PANE`, config `ab.lua`, `cli --no-auto-start list --format json`) | result (workspaces in JSON) | exit |
|---|---|---|
| `WEZTERM_UNIX_SOCKET=/tmp/dmux-s1/b.sock` (B, despite A first in config) | ONLY `SPIKE-WS-B`, `B-default` | 0 |
| `WEZTERM_UNIX_SOCKET=/tmp/dmux-s1/a.sock` (inverted) | ONLY `SPIKE-WS-A`, `A-default` | 0 |
| `WEZTERM_UNIX_SOCKET=/tmp/dmux-s1/b.sock` with config `a.lua` that lists ONLY domain A | ONLY `SPIKE-WS-B`, `B-default` | 0 |

Trimmed JSON row (A list): `{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"SPIKE-WS-A",...,
"title":"sleep","cwd":"file:///Users/fredrir/","tty_name":"/dev/ttys007"}` plus
`{"window_id":0,...,"workspace":"A-default","tty_name":"/dev/ttys004"}`. B symmetric
(`SPIKE-WS-B`/`B-default`). Zero cross-contamination in any run.

Third row is the decisive one: **the env var is the sole endpoint selector; the config file does not
participate in selection at all** (no matching `unix_domains` entry needed). `--prefer-mux` never used.

## Task 3 — Typed failure classification (stock CLI)

All runs: `env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=<path> wezterm --config-file
/tmp/dmux-s1/ab.lua cli --no-auto-start list --format json`. `pgrep -fl wezterm` diffed before/after
each: **`--no-auto-start` spawned nothing in every case** (`NO-NEW-PROCESSES`).

| case | setup | exit | stdout | stderr (exact) | stock-distinguishable? |
|---|---|---|---|---|---|
| (a) path absent | never created | 1 | empty | `ERROR wezterm > failed to connect to Socket("/tmp/dmux-s1/absent.sock"): connecting to /tmp/dmux-s1/absent.sock; terminating` | no — identical text to (b),(d) |
| (b) stale socket | `kill -9` server B, `b.sock` left on disk | 1 | empty | `ERROR wezterm > failed to connect to Socket("/tmp/dmux-s1/b.sock"): connecting to /tmp/dmux-s1/b.sock; terminating` | no |
| (c) live non-wezterm UDS (python listener sending a garbage banner) | `fake.sock` | 1 | empty | `ERROR wezterm_client::client > Error while decoding response pdu: decoding a PDU: Corrupt Response: decode_raw_async: serial 79 is implausibly large (bigger than 2)` + same line from `wezterm` `; terminating` | yes — unique "Corrupt Response" |
| (c2) live non-wezterm UDS that accepts and sends NOTHING | `silent.sock` | — | — | **CLI HANGS >12 s, no output, no built-in timeout; had to be killed** | n/a — maps to `timeout` outcome only if dmux imposes a deadline |
| (d) regular file at socket path | `touch plain.sock` | 1 | empty | `ERROR wezterm > failed to connect to Socket("/tmp/dmux-s1/plain.sock"): connecting to /tmp/dmux-s1/plain.sock; terminating` | no |

`WEZTERM_LOG=debug` adds no errno / cause chain to (a)/(d) — the anyhow context is dropped from the log
line. So stderr parsing alone cannot produce §8.1's typed outcomes for (a)/(b)/(d).

### dmux-side probe closes the gap (demonstrated)

Python probe (lstat/stat -> S_ISSOCK -> connect(2) with 3 s timeout -> `getsockopt(SOL_LOCAL=0,
LOCAL_PEERPID=2)`), run against all five paths:

```
/tmp/dmux-s1/absent.sock  => ENOENT            -> "owner-proven … never started/unpublished" class
/tmp/dmux-s1/b.sock       => ECONNREFUSED      -> stale socket, server dead
/tmp/dmux-s1/plain.sock   => not-a-socket      -> ENOTSOCK class, no connect attempted
/tmp/dmux-s1/a.sock       => connect OK, LOCAL_PEERPID=34842 (== service pid), dev=16777231 ino=5926384
/tmp/dmux-s1/fake.sock    => connect OK, LOCAL_PEERPID=35351 (!= service pid -> wrong process)
```

The imposter case (c) is caught two independent ways: pre-flight `LOCAL_PEERPID` vs the
service-recorded server pid (macOS; `SO_PEERCRED` on Linux), and — if it ever got past that — the stock
CLI's own unique `Corrupt Response` stderr / absent sentinel in `list`.

## Task 4 — Socket-replacement race + detection

1. Sentinel planted in A via exact socket: `cli --no-auto-start spawn --new-window --workspace
   "dmux:system:1786880224" -- /bin/sleep 3600` (exit 0, pane 2).
2. Published path: `ln -s /tmp/dmux-s1/a.sock /tmp/dmux-s1/cur.sock`. Publish-time identity recorded:
   `stat -L -f 'dev=%d ino=%i type=%HT'` -> `dev=16777231 ino=5926384 type=Socket`.
3. `list` via `WEZTERM_UNIX_SOCKET=/tmp/dmux-s1/cur.sock` -> `['A-default','SPIKE-WS-A','dmux:system:1786880224']`, exit 0.
4. **Atomic repoint** (rename(2)): `ln -s /tmp/dmux-s1/b.sock cur.sock.new && mv -f cur.sock.new cur.sock`.
5. `list` via the SAME path -> `['B-default','SPIKE-WS-B']`, **exit 0, zero warnings — silent wrong-server
   rows confirmed.** Path identity is worthless as instance identity.
6. **Detection 1 (stat):** pre-op `stat -L` -> `dev=16777231 ino=5926385` ≠ recorded `ino=5926384` =>
   `SWAP-DETECTED-BY-STAT` (`wrong_backend_instance`).
7. **Detection 2 (sentinel, end-to-end on the same list the op would use):** post-swap list lacks
   `dmux:system:1786880224` => `WRONG-BACKEND-INSTANCE-DETECTED`. This check is immune to stat TOCTOU
   because it rides in the very JSON response consumed by inventory.

Both proposed mechanisms work; sentinel is the authoritative one, stat + LOCAL_PEERPID are the cheap
pre-flight tier. dmux should also record and compare the *resolved* real path at publish time.

## Task 5 — Identity/handshake surface of stock CLI

- `wezterm cli --help`: only `--no-auto-start`, `--prefer-mux`, `--class`. No socket-path flag (env var
  is the only exact-endpoint input), no "print server version/pid" subcommand.
- `list --format json` fields: window_id/tab_id/pane_id/workspace/size/title/cwd/cursor*/tab_title/
  window_title/is_active/is_zoomed/tty_name. **No server identity fields.**
- `list-clients --format json` against A returned `[]` (the CLI's own transient connection is not
  reported); no server pid/version either.
- `WEZTERM_LOG=wezterm_client=debug,codec=debug,debug` on a successful list logs only
  `codec > encode_async ListPanes size=3` — the internal GetCodecVersion handshake result (server
  version string) is NOT surfaced anywhere.
- => Server identity must come from dmux: `LOCAL_PEERPID` probe (demonstrated), service-recorded start
  token, stat dev/ino, and the sentinel workspace. All demonstrated above.

## Risks / unknowns

- The silent-listener hang means every stock-CLI child needs a hard dmux-side deadline (kill on expiry)
  or a healthy pre-flight probe first; the plan's `timeout` outcome is dmux-manufactured, not wezterm's.
- Stock stderr text (`failed to connect`, `Corrupt Response`) is not a stable API; classification must
  come from the dmux probe, with stderr kept only as diagnostic payload.
- ~104-byte `sun_path` limit constrains the service's published socket path; publish short real paths.
- TOCTOU window between probe and CLI child remains (probe pid check ≠ same connection the CLI uses);
  sentinel-in-list on the operation's own response is the closing check, per plan §8.1.
- `wezterm cli` resolves `WEZTERM_UNIX_SOCKET` through symlinks (connect(2) semantics); dmux should
  record the resolved path + dev/ino at publish and re-verify pre-op.
- Version skew CLI<->server (plan's `version mismatch`) not exercised here (single build available);
  codec handshake exists internally but is not observable — a skewed server likely surfaces as
  `Corrupt Response`/protocol error class.

## Cleanup

Spawned pids (recorded in spike1/pids.txt): 34842 (mux A), 34843 (mux B, killed during case (b)),
35351 (fake listener), 35486 (silent listener) — all killed; scratch sockets removed; `/tmp/dmux-s1`
symlink removed. Live GUI pid 9640 and other spike agents' servers untouched.
