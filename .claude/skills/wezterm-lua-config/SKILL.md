---
name: wezterm-lua-config
description: Conventions, module contract, and the shared validation loop for the WezTerm Lua
  configuration in this dotfiles repo (shared/wezterm/**) — appearance, keybindings, plugins,
  integrations, and the tests and static checks covering all of it. Use when editing, adding,
  reviewing, formatting, or testing Lua here, and whenever a Lua change needs verifying before
  commit. Covers the apply/setup contract, module load order, the managed-mode key sanitizer,
  the DMUX_WEZ_FIRST gate, stylua and luacheck house style, and the exact test commands. Also
  fires on "run the wezterm tests", "add a test for this Lua module", "why did my keybinding
  disappear", or "format this config". For the signed bridge protocol under wez/dmux_bridge use
  dmux-bridge-actions; for mux/dmux-mux.lua, wez/domains/init.lua, wez/plugins/resurrect.lua and
  the service units use dmux-mux-lifecycle.
compatibility: Needs a Lua 5.4-compatible interpreter on PATH for the test suite, plus stylua
  and luacheck for the static checks. Two suite cases stay skipped without a WezTerm fork
  checkout.
metadata:
  version: "1"
---

# WezTerm Lua config

Paths are relative to the repo root. The config lives in `shared/wezterm/`;
`shared/wezterm/types/` is a vendored, gitignored LuaLS stub checkout — never edit or lint it.

## Validate every change

Run all three. Two have a non-zero baseline you need to know about, or you will chase pre-existing
noise.

```bash
sh shared/wezterm/wez/dmux_bridge/tests/suite.sh
```

Requires `0 failed`. The only expected skips are `show_keys_config` (a fixture, not a test) and two
cases needing a real WezTerm fork checkout (`DMUX_WEZTERM_SOURCE`, `DMUX_WEZTERM_BIN`). Any `FAIL`
is yours. The repo-wide runner wraps the same suite as `bash tests/run.sh dmux-lua`.

```bash
cd shared/wezterm && stylua --check .
```

Exits 1 on one pre-existing diff, `wez/design.lua`. Your files must not add a second. Run
`stylua .` from `shared/wezterm` to fix formatting; `.styluaignore` excludes `wez/theme.lua` and
`types/`.

```bash
cd shared/wezterm && luacheck wezterm.lua wez mux
```

There is a standing backlog of warnings and zero errors. Don't add warnings; don't try to reach
zero. Compare against the count on a clean tree rather than assuming any number.

## Module contract

`shared/wezterm/wezterm.lua` drives every module through the same two optional entry points:

```lua
local mod = require(name)
if mod.apply then mod.apply(config) end
if mod.setup then mod.setup() end
```

`apply(config)` mutates the config table. `setup()` is the only place `wezterm.on` may be called.
Splitting them this way is what lets the config be evaluated in tests without registering handlers.

A module is a table: `local M = {}` … `return M`. Leaf key modules export data
(`M.keys`, `M.key_tables`) and never touch `config`; `wez/keys/init.lua` aggregates them.

Module load errors are logged and swallowed so one broken feature can't drop the whole config to
stock defaults — with two deliberate exceptions in `wezterm.lua`: a `wez.domains` failure under
`DMUX_WEZ_FIRST=1` is re-raised, and `dmux_bridge.apply/setup` run outside the `pcall` entirely.
Both are fail-closed on purpose. Don't "fix" them by adding a `pcall`.

## Managed mode rebuilds your keybindings

`DMUX_WEZ_FIRST=1` is read with `os.getenv` at config-evaluation time in fifteen files; the GUI
binary of the maintained fork tests the same literal `1`, so the Lua keeps `== '1'` even though
Wez-first is the default since the §21 step 9 flip — the `1` reaches the GUI from `dmux _gui
summon`/`--launch-gui`, or from the launchd session where `dmux-env-load.sh` places the per-host
value or the tracked default. When set,
`wez/dmux_bridge/init.lua` runs `preflight` before every other module and then, after all of them
(plugins included), **rebuilds `config.keys` from an allowlist of sources**, in this order:

1. `wez.keys.leader`, `wez.keys.copy`, `wez.keys.window`, `wez.keys.mac` via `append_keys`
2. `actions.keys()`
3. `actions.mac_keys()`, only when `platform.is_mac`
4. one literal binding — `LEADER+w` → `picker.action()` from `wez.plugins.workspace_picker`

`config.key_tables` is likewise rebuilt from `actions.key_tables()` alone, and `config.launch_menu`
and `config.mouse_bindings` are emptied. So a binding you `table.insert` into `config.keys` from
anywhere else is silently discarded in managed mode. To add one, put it in an allowlisted module or
in `wez/dmux_bridge/actions.lua`.

The allowlist is over modules rather than chords because a chord allowlist would keep an unsafe
action if a plugin swapped the action behind the same chord.

Leaf modules that offer unsafe native actions self-neuter at the top instead of being filtered:

```lua
if os.getenv 'DMUX_WEZ_FIRST' == '1' then
  M.keys = {}
  M.key_tables = {}
  return M
end
```

## House style

`stylua.toml` sets `call_parentheses = "None"`, so single string/table arguments drop parens —
`require 'wezterm'`, `os.getenv 'DMUX_WEZ_FIRST'`, `error 'message'`, `done { pong = true }`.
Write it that way or stylua rewrites your diff. Also: 2-space indent, 120 columns,
`quote_style = "AutoPreferSingle"` (single quotes unless doubles avoid an escape), `snake_case`,
`SCREAMING_SNAKE` constants.

Order inside a file: requires, `local M = {}`, constants, `local function`s, `function M.x()`,
`return M`. Private-by-default — `presentation.lua` exports one function out of thirty.

Comments are sparse and almost never say *what* the code does. They record a decision, a hazard, or
a rejected alternative. Match that: a comment restating the code will read as noise here.

LuaLS `---@` annotations are rare — among the thirteen bridge modules only `inventory.lua` carries
any. Annotate where a contract is subtle or a value is nil-able; otherwise plain `--` prose is the
convention. Optional fields are written `T|nil`, not `T?`.

## Gotchas

- Three sources disagree on the Lua version: `.luarc.json` says 5.4, `.luacheckrc` says
  `std = luajit` (5.1), and the installed `lua` may be newer. **WezTerm embeds Lua 5.4** — that is
  the one that governs. luacheck's std will not flag 5.3+ syntax problems, and it has no entry for
  `utf8`, which `json.lua` uses.
- Six bridge modules must never `require 'wezterm'` at module scope — `canonical`, `crypto`,
  `json`, `protocol`, `context`, `correlation`. The unit tests run them under a plain `lua` binary
  with no WezTerm at all; adding that require breaks the whole suite.
- `wezterm.run_child_process` *raises* when the program is missing rather than returning a failure,
  so every call site wraps it in `pcall`. It also cannot supply stdin — pass data through a file in
  the runtime dir positionally.
- An error inside a `wezterm.on` handler or a `wezterm.time.call_after` callback propagates nowhere
  useful, because each is a fresh unprotected entry from Rust. Every handler re-establishes its own
  `pcall` inside the callback body.
- There are no coroutines anywhere in this config. Deferred work is `wezterm.time.call_after`
  rescheduling itself; blocking `wezterm.sleep_ms` spins exist only in `mux/dmux-mux.lua`.
- `wezterm.GLOBAL` is process-shared across config generations and is deliberately *not* the
  authority for handing state from config evaluation to `gui-startup`. Use it for diagnostics and
  idempotence latches (`wezterm.GLOBAL.dmux_bridge_events_registered`), not as a source of truth.
- Managed configs pin `automatically_reload_config = false`, so editing a Lua file does not
  hot-reload a running managed GUI. Restart it to see a change.
- `io`, `os.execute`, and `os.getenv` are all available — WezTerm does not strip the stdlib. But
  `io.popen` is never used, and it is explicitly banned in `mux/dmux-mux.lua`.

The normative product spec for anything dmux-related is `docs/dmux-wezterm-first-plan.md`.

## Reference files

- `references/lua-runtime-facts.md` — Lua 5.4 semantics this config actually depends on: the two
  meanings of `~`, integer-vs-float formatting on the wire, byte-oriented strings, unspecified
  `pairs` order, and a confirmed range-guard hole. Read before editing `canonical.lua`, `json.lua`,
  `crypto.lua`, or any code that formats numbers into a signed document or compares byte strings.
- `references/test-harness.md` — how the suite is structured, the `mode_for` registration
  requirement, WezTerm stubbing patterns, the assertion vocabulary, and cross-scenario state
  hazards. Read before adding or modifying anything under `wez/dmux_bridge/tests/`.
