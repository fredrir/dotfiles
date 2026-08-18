# Marker correlation: owner refs → GUI-local objects

`shared/wezterm/wez/dmux_bridge/correlation.lua`, with the marker parser in `context.lua`.

## Contents
- Why correlation exists
- The marker schema
- Match dispositions
- Workspace precondition
- Group-only tiebreak
- focus_pane's stronger match

## Why correlation exists

Imported Wez domains can remap IDs, so owner-server tab/pane IDs must never be applied to the GUI.
The owner returns epoch-qualified logical child refs; the GUI re-derives which of *its* tabs and
panes those refer to by reading `DMUX_GROUP_REF`/`DMUX_SPLIT_REF` back off each pane's user
variables, and activates only the correlated GUI-local object.

`M.resolve` proves the workspace exists before anything is activated. `M.activate` never calls
`mux.set_active_workspace` unless `resolve` returned first — a check followed by
`SwitchToWorkspace` would be insufficient, because the switch may itself create the workspace and
violate con-never-creates.

## The marker schema

`context.FIELD_MAP` maps the nine user variables of `docs/dmux-wezterm-first-plan.md` §13.1 1:1:
`dmux_context_version`,
`dmux_host_uid`, `dmux_space_uid`, `dmux_space_no`, `dmux_backend`, `dmux_domain`,
`dmux_server_epoch`, `dmux_group_ref`, `dmux_split_ref`.

Validation, each returning a typed refusal:

- `missing_marker` — no user vars at all, or a named field absent
- `marker_version` — `dmux_context_version` must be exactly `'1'`
- `malformed_marker` — non-UUID identity/epoch, `dmux_space_no` not canonical nonzero decimal
  (`'^[1-9][0-9]*$'`), backend not `wez`/`tmux`, bad child ref, invalid pane id
- `marker_domain_mismatch`, `marker_epoch_mismatch`, `marker_backend_mismatch`

`dmux_domain` is the one field allowed to be empty — it was empty in the P8 bootstrap payload. When
populated it is an additional equality check, never a source to guess from.

`dmux_tmux_client_uid` is read but deliberately absent from `FIELD_MAP`: it is an untrusted
locator, bound by Rust to a private owner record plus one exact live tmux client before any
tmux-originated GUI action is accepted.

Space-scope equality is `context.matches_target`, comparing `host_uid`, `space_uid`, `server_epoch`,
and `gui_domain` — note `gui_domain` (the imported domain the pane actually arrived through), not
`marker.domain`.

## Match dispositions

Every level fails closed on both zero and many.

| Level | 0 matches | exactly 1 | more than 1 |
|---|---|---|---|
| workspace → window | `not_found` "opaque workspace is not imported in this GUI" | proceed | `ambiguous_workspace` |
| panes in workspace | `not_found` "opaque workspace has no panes" | — | — |
| Space/epoch in workspace | `workspace_context_mismatch` | proceed | — |
| Group ref → panes | `group_not_found` | — | fine if all in one tab |
| Group ref → tabs | — | proceed | `ambiguous_group` |
| Split ref → panes in Group | `split_not_found` | focus it | `ambiguous_split` |
| Group-only, active panes | fall through to tiebreak | keep it | `ambiguous_split` "the Group reports more than one active matching Split" |
| `focus_pane` exact marker | `pane_not_found` | focus | `ambiguous_pane` |

A Group ref may legitimately appear on several panes, but they must all belong to exactly one GUI
tab. A Split ref must match exactly one pane, in its parent Group.

## Workspace precondition

Before any of the above, `validate_workspace_context` walks every pane in the window and refuses if:

- a pane has no valid marker → `invalid_marker`, "opaque workspace contains an unstamped or
  malformed pane"
- a pane's `gui_domain` differs from the target domain → `workspace_domain_mismatch`, "contains a
  pane imported through another GUI domain"
- the workspace has no panes → `not_found`
- no pane belongs to the requested Space and epoch → `workspace_context_mismatch`

Matching uses `tab:panes_with_info()` rather than `tab:panes()` specifically to get `is_active`.

## Group-only tiebreak

With a Split target, that pane is focused. With a Group target only:

1. If exactly one matching pane reports `is_active`, keep it — this preserves what the user was
   looking at.
2. More than one active match is `ambiguous_split`.
3. Otherwise sort matches by `marker.split_ref` and take the lexicographically smallest.

The sort key is the canonical Split ref, not a remapped pane id — pane ids differ between owner and
GUI, so ordering by them would be non-deterministic.

## focus_pane's stronger match

`focus_pane` is a no-create, no-attach operation on an already-visible outer GUI pane, and demands
a strictly stronger match than Group/Split activation — eleven fields including `gui_pane_id`,
`tmux_client_uid`, `backend == 'tmux'`, and `domain == nil`.

It also snapshots the whole mux inventory before and after and refuses if anything moved:

- rows differ → `pane_inventory_changed`, "GUI pane inventory changed during no-create focus"
- the match is no longer unique → `focus_postcondition_failed`

A pane id alone is never sufficient: the complete marker plus attach-time client UID must identify
exactly one pane both before *and* after activation.
