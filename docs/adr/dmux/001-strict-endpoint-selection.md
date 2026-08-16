# ADR 001: Strict Wez endpoint selection (P0 spike 1)

Status: accepted (P0 evidence; mechanism selected)
Date: 2026-08-16
Evidence: repo `docs/adr/dmux/evidence/spike1-strict-endpoint.md` (spike run, wezterm 20260813-114614-18a44cb7)
Plan refs: §8.1, §11.1

## Decision

Stock `wezterm cli` plus dmux-side verification is the selected mechanism.
No forked strict selector is required. Frozen invocation template:

```text
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE \
  WEZTERM_UNIX_SOCKET=<exact-service-socket> \
  wezterm --config-file <dmux-managed-config> cli --no-auto-start <subcmd> [--format json]
```

Mandatory dmux-side rules that make this strict:

1. **Deadline on every CLI child.** A live-but-silent socket hangs `wezterm cli`
   for >12s with no built-in timeout; the plan's `timeout` outcome is produced
   by dmux killing the child, never by wezterm.
2. **Pre-flight identity probe before trusting any output:** `lstat`/`stat` the
   socket path, `connect(2)` and classify errno, then read `LOCAL_PEERPID`
   (macOS; `SO_PEERCRED` on Linux) and compare against the service-recorded
   PID/start token.
3. **Sentinel-in-list as the TOCTOU-immune check:** the reserved
   `dmux:system:<epoch>` workspace must be present in the same `list` JSON the
   operation consumes. The stat/dev-ino check detects socket replacement at
   probe time; the sentinel check detects it end-to-end inside the very
   response being used.

## Proven facts

- `WEZTERM_UNIX_SOCKET` is the **sole** endpoint selector: with a config
  listing only domain A, env socket B still returned exclusively B's rows.
  Config order and `--prefer-mux` play no part.
- `unix_domains = {{ name = ..., socket_path = ... }}` in a per-instance
  `--config-file` binds exactly that socket (lsof-verified, no default runtime
  socket created).
- `--no-auto-start` spawned zero processes in every failure case tested.
- Silent wrong-server risk is real: after an atomic symlink repoint of the
  recorded path from server A to server B, the same command returned B's rows
  with exit 0 and no warning. Both detections above caught it
  (dev/ino change; sentinel absent from the post-swap list).

## Failure classification (stock exit codes/stderr are NOT a typed API)

| Observed case | wezterm behavior | dmux typed outcome (via own probe) |
| --- | --- | --- |
| Socket path absent | exit 1, generic text | candidate `stopped` (owner-local proof still required per §8.1) |
| Stale socket file (server killed) | exit 1, identical generic text | `stopped`/crashed instance |
| Regular file at path | exit 1, identical generic text | endpoint invalid (`malformed`) |
| Live non-wezterm socket (talks) | exit 1, `Corrupt Response: decode_raw_async ...` | `protocol mismatch`/`malformed` |
| Live socket, silent | hangs indefinitely | `timeout` (dmux-imposed deadline) |
| Version skew | not exercisable single-build; expected `Corrupt Response` class | `version/protocol mismatch` (verify in P7 matrix) |

Errno-level classification (ENOENT / ECONNREFUSED / ENOTSOCK / connect-OK +
peer-pid mismatch) was demonstrated working and is the normative source of
typed outcomes; wezterm stderr text is diagnostics only.

- Stock CLI exposes **zero** server identity: no version banner, no server
  pid; `list-clients` omits the CLI's own connection. Instance identity is
  entirely dmux-manufactured: peer-pid + service start token + dev/ino +
  sentinel epoch.

## Design consequences

- The service descriptor (§15.1) must record: exact socket path, socket
  dev/ino, server PID + start token, backend-instance UID, epoch. All were
  exercised as detection inputs.
- **Short socket paths are mandatory** (macOS `sun_path` ~104 bytes). The
  service publishes sockets under a short runtime dir; `dmux_runtime_dir()` on
  macOS (`_CS_DARWIN_USER_TEMP_DIR`, typically `/var/folders/...`) satisfies
  this, but the length check becomes a doctor-level validation.
- A fresh `wezterm-mux-server` **auto-spawns pane 0** in `default_workspace`
  when started without a `mux-startup` handler — a bare fresh inventory is
  never empty. Sentinel/default-suppression (spike 2 / ADR 002) is therefore
  load-bearing for the "empty server" concept, not cosmetic.
