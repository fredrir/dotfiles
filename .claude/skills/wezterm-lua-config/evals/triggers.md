# Trigger cases

Re-check these after any edit to the `description`. Run each prompt in a fresh session and record
which skill loads.

## Should fire

| Prompt | Why |
|---|---|
| "run the wezterm tests" | names the validation loop this skill owns |
| "format this lua file" | stylua conventions |
| "add a test case for the picker" | test harness + `mode_for` registration |
| "why did my LEADER+w binding disappear?" | managed-mode key sanitizer |
| "does the wezterm config still load after this change?" | module contract + swallow/re-raise rules |
| "clean up the luacheck warnings in wez/appearance" | static-check baseline |
| "add a status line segment" | appearance module, no bridge protocol change |

## Should not fire alone

| Prompt | Expected owner | Failure mode if this skill wins |
|---|---|---|
| "add a new bridge action" | `dmux-bridge-actions` | agent misses the `ACK_KEYS` latch and ships a bridge that dies on first crash-recovery |
| "the mux service won't start" | `dmux-mux-lifecycle` | agent debugs Lua style instead of the runtime descriptor |
| "restore my workspaces after reboot" | `dmux-mux-lifecycle` | agent edits resurrect opts without the empty-server guard |

## Known acceptable co-fire

"add a keybinding that calls dmux" reasonably loads this skill *and* `dmux-bridge-actions` — the
sanitizer rule lives here, the verb vocabulary lives there. Both are needed; this is not a misfire.
