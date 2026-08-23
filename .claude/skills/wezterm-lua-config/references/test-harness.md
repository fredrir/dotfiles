# The dmux GUI Lua test suite

Tests live in `shared/wezterm/wez/dmux_bridge/tests/`. They run under a plain `lua` binary with no
WezTerm present.

## Contents
- Structure: suite.sh is the entry point, not run.lua
- Registering a new case in `mode_for`
- Running one case by hand
- Stubbing WezTerm
- Injecting a mux tree
- Assertions
- Minimal skeleton
- Cross-scenario state hazards

## Structure: suite.sh is the entry point, not run.lua

`tests/suite.sh` shells out to `lua <file>.lua` once per case, with per-case environment
preconditions. `tests/run.lua` is **one case among twenty** (covering crypto, protocol, context,
correlation) — it is not a harness and does not load the other files.

There is no shared assertion library, no registration table, and no `return {}` contract. Each case
is a top-level script whose assertions run at load, ending with exactly one `io.stdout:write`
success line. That line is the pass signal; `suite.sh` captures and discards it unless the case
fails.

`suite.sh` computes its own working directory and `cd`s to the repo root, so it can be invoked from
anywhere. That matters because every case prepends **relative** patterns to `package.path`:

```lua
package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')
```

`LUA_PATH` is never used. Override the interpreter with `DMUX_TEST_LUA=/path/to/lua`.

## Registering a new case in `mode_for`

Discovery is automatic but **an unlisted `.lua` file reports FAIL, not skip**. This is deliberate:
"a test nobody runs is how the leaked connection-UI domain survived a suite that already covered
rogue domains."

After creating `tests/<name>.lua`, add `<name>` to the right arm of `mode_for()` in `suite.sh`:

| Mode | Preconditions | Existing cases |
|---|---|---|
| `unit` | no dmux environment at all | actions, consumer, controller, instance, mux_startup_witness, presentation, run |
| `managed` | `DMUX_WEZ_FIRST=1` + a private `DMUX_RUNTIME_DIR` | config, config_linux, domains, picker, remote, resident_ingress, status, top_level, top_level_missing_descriptor, top_level_missing_key |
| `flag-off` | asserts `DMUX_WEZ_FIRST` is absent | config_off, top_level_off |
| `matrix` | parameterised; run once per combination | hammerspoon |
| `fixture` | not a standalone test | show_keys_config |

Managed cases each get a fresh `mktemp -d` runtime dir that is removed afterwards, because
`top_level_missing_descriptor` writes a descriptor into it *after* asserting its absence. Sharing
one directory across cases breaks that case.

`matrix` cases are skipped by the main loop and driven by an explicit nested loop; `hammerspoon`
covers an eighth of itself per invocation across `state × managed × frontmost`.

## Running one case by hand

Replicate the declared mode. From the repo root:

```bash
env -u DMUX_WEZ_FIRST -u DMUX_RUNTIME_DIR -u HAMMER_APP_STATE -u HAMMER_FRONTMOST \
  lua shared/wezterm/wez/dmux_bridge/tests/run.lua
```

```bash
rt=$(mktemp -d) && env -u HAMMER_APP_STATE -u HAMMER_FRONTMOST \
  DMUX_WEZ_FIRST=1 DMUX_RUNTIME_DIR="$rt" \
  lua shared/wezterm/wez/dmux_bridge/tests/config.lua; rm -rf "$rt"
```

Use `env -u` rather than merely not setting the variable: `DMUX_WEZ_FIRST` is exported by a managed
GUI's own shell, so a flag-off case run by hand from a managed terminal fails without the scrub.

## Stubbing WezTerm

Every case builds its own `fake_wezterm` and installs it — there is no shared stub module:

```lua
package.preload.wezterm = function()
  return fake_wezterm
end
```

Use `package.preload` for modules not yet loaded (the factory runs lazily, so it can `error` to
assert a module must *not* load). Use `package.loaded[name] = M` when you want a mutable handle to
patch later.

`wezterm.action` is a metatable turning any name into a constructor:

```lua
local act = setmetatable({ PopKeyTable = { name = 'PopKeyTable' } }, {
  __index = function(_, name)
    return function(value)
      return { name = name, value = value }
    end
  end,
})
```

Pre-seed the unit-variant actions that are tables rather than functions (`PopKeyTable`,
`HideApplication`, `QuitApplication`) or indexing them returns a constructor you can't compare.

`wezterm.action_callback` has two conventions — wrap (`{ name = 'Callback', callback = cb }`, then
invoke `binding.action.callback(window, pane)`) or identity (`return cb`, then invoke
`picker.action()(window, pane)`). Pick per what you assert on.

`wezterm.emit` is never faked and never used. Capture handlers into a local table instead:

```lua
on = function(name, callback)
  events[name] = callback
end,
```

then call `events['update-status'](window, pane)` directly.

`wezterm.time.call_after` becomes a queue you drain by hand:

```lua
time = {
  call_after = function(_, callback)
    table.insert(scheduled, callback)
  end,
},
```

then `table.remove(scheduled, 1)()` to advance. For timeout paths, monkey-patch `os.time` to a
frozen `fake_now` and restore it from a saved `real_time` afterwards.

`wezterm.log_error` has two conventions: collect into a table when you want to assert on the exact
message, or `error(message)` when any logged error should be a hard failure.

`wezterm.home_dir = '/tmp'` is what makes the resolved binary path `/tmp/.local/bin/dmux`.
`wezterm.target_triple` is the platform switch — `config_linux.lua` flips one line to
`'x86_64-unknown-linux-gnu'` and additionally preloads `wez.platform` as `{ is_mac = false }`.

**Booby-trapped stubs are assertions, not oversights.** Several deliberately raise:

```lua
dmux_bridge_open = function()
  error 'config test must not acquire a bridge lease'
end,
```

Making one of these return a value deletes the assertion.

## Injecting a mux tree

Panes, tabs, and windows are plain closure-tables with method fields. The reusable factory triple
is at the top of `run.lua` — `pane(id, vars, domain)`, `tab(id, pane_infos)`,
`window(id, workspace, tabs)`.

The primary injection mechanism is **reassigning `all_windows`** mid-test, or holding a `windows`
upvalue that a stable closure reads:

```lua
local mux = {
  all_domains = function() return rows end,
  all_windows = function() return windows end,
  set_active_workspace = function(name) active_workspace = name end,
}
-- later: windows = { sentinel_window, target_window }
```

Domains must be an **ordered array**, not a name-keyed table: the real mux keys by domain_id and
hands Lua an ordered array, so two rows may legitimately share a name, and `pairs` order is
unspecified anyway. A domain created without a capability has no `is_spawnable` method at all — that
is the shape of a domain object that predates or omits it, and the code must tolerate it.

Use `tab:panes_with_info()` when the test needs `is_active`; `tab:panes()` does not supply it.

## Assertions

19 of 20 cases use bare `assert(cond, message)`. Only `run.lua` defines helpers, and they are
file-local:

```lua
local function equal(actual, expected, label)  -- ~= comparison, error level 2
local function truthy(value, label)
local function error_code(_, err) return err and err.code end
```

`error_code` is not an assertion — it adapts the pervasive `(result, err)` convention so
`equal(error_code(f(...)), 'malformed_request', '…')` reads naturally.

There is no `expect_error`. Negative paths use either the typed-error convention above, or
`pcall` plus a pattern match on the message:

```lua
local ok, err = pcall(bridge.apply, config)
assert(not ok and tostring(err):match 'no canonical backend instance')
```

The negative-path convention worth copying: assert the side effects that must **not** have
happened, not just the raised error. The `top_level_*` cases all end by asserting the GLOBAL fields
stayed `nil`, and `top_level_missing_key` counts module loads to prove the abort happened first.

## Minimal skeleton

```lua
package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')

local events = {}
local fake_wezterm = {
  GLOBAL = {},
  on = function(name, callback)
    events[name] = callback
  end,
}
package.preload.wezterm = function()
  return fake_wezterm
end

package.loaded['wez.dmux_bridge.controller'] = {
  run = function()
    error 'this test must not reach the controller'
  end,
}

local subject = require 'wez.<module under test>'
subject.setup()
assert(type(events['update-status']) == 'function')

io.stdout:write 'dmux <name> test: <what it pinned> passed\n'
```

Then add the basename to `mode_for`.

## Cross-scenario state hazards

Each case is one Lua process, so there is total isolation *between* files and none *within* one.
Consequences:

- Scenario order is load-bearing. `package.preload`/`package.loaded` mutations, `os.time`
  overrides, and `fake_wezterm.GLOBAL` persist for the whole file. An assertion inserted mid-file
  inherits whatever the previous block left behind.
- Mutations must be manually restored. The pervasive shape is save → mutate → assert → restore, for
  module functions, stub methods, and `wezterm.gui` fields alike. Forget the restore and every
  later block silently tests the wrong thing.
- Re-`require` needs an explicit cache bust: `package.loaded['wez.dmux_bridge.consumer'] = nil`
  before re-requiring.
- `consumer.lua`'s `new_harness()` reassigns shared globals as a side effect, so older harness
  objects stop receiving anything. Interleaving two harnesses does not work.
- Managed cases really do touch the filesystem — they `mkdir` and write a key under
  `$DMUX_RUNTIME_DIR`.
- `show_keys_config.lua` requires a *real* `wezterm` and fails under plain `lua`; it is a
  `--config-file` fixture for `managed_show_keys.sh`, which is why `mode_for` has a `fixture` arm.
- `fork_surface.sh` pins exact occurrence counts in the Rust fork and needs `rg` and `awk`. It
  breaks on upstream refactors that are otherwise harmless.
