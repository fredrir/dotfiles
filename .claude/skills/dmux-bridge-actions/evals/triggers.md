# Trigger cases

Re-check these after any edit to the `description`. Run each prompt in a fresh session and record
which skill loads.

## Should fire

| Prompt | Why |
|---|---|
| "add a new bridge action" | the core workflow |
| "why is the bridge rejecting this request?" | typed refusal vocabulary |
| "the HMAC doesn't match after I added a field" | signing document + allowlists |
| "make Command+W remove the group instead" | `actions.lua` confirm flow |
| "focus the right pane after attaching" | correlation rules |
| "add a field to the acknowledgement" | `ACK_KEYS` — the highest-value gotcha here |
| "cold launcher can't attach the domain" | origin kinds |

## Should not fire alone

| Prompt | Expected owner | Failure mode if this skill wins |
|---|---|---|
| "run the wezterm tests" | `wezterm-lua-config` | loads 5k tokens of protocol detail for a one-line command |
| "format this lua file" | `wezterm-lua-config` | same |

## Regression guard

If a change to this description starts pulling in "format", "lint", or "run tests" prompts, the
scope sentence has drifted toward general Lua work. Narrow it back to the protocol surface.
