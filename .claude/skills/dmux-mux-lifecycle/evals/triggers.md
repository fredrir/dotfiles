# Trigger cases

Re-check these after any edit to the `description`. Run each prompt in a fresh session and record
which skill loads.

## Should fire

| Prompt | Why |
|---|---|
| "the mux service won't start" | descriptor + service ownership |
| "why is there a DMUX-CANARY pane?" | default-prog suppression tripwire |
| "restore my workspaces on boot" | resurrection split + recovery eligibility |
| "my panes vanished after restarting wezterm" | attach-vs-restart distinction |
| "edit the systemd unit for the mux server" | service units |
| "wezterm reconnected but everything was empty" | empty-server guard, intentional-empty revision |
| "change what mux-startup does" | the in-process-only rules |

## Should not fire alone

| Prompt | Expected owner | Failure mode if this skill wins |
|---|---|---|
| "add a bridge action" | `dmux-bridge-actions` | agent reads recovery protocol for an unrelated change |
| "run the wezterm tests" | `wezterm-lua-config` | oversized load for one command |
| "why did my keybinding disappear?" | `wezterm-lua-config` | agent looks at service startup instead of the sanitizer |

## Safety check

Any prompt that reaches the service-restart commands should also surface the warning that a restart
kills live panes. If that clause ever moves out of `SKILL.md` into a reference file, this skill has
regressed — the agent must not need a second file read to learn the command is destructive.
