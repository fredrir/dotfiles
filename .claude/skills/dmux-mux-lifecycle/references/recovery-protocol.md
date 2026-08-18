# Guarded cold recovery

Normative source: `docs/dmux-wezterm-first-plan.md` §15.3. Implementation:
`shared/wezterm/mux/dmux-mux.lua`, with the Rust coordinator in
`scripts/rust/crates/dmux/src/recovery.rs`.

## Contents
- What recovery is and is not
- Why mux-startup is the only trigger
- The algorithm
- Descriptor and generation states
- Manifest eligibility
- Crash, resume, and abort
- Snapshot exclusion

## What recovery is and is not

Recovery reconstructs eligible shells, layout, cwd, titles, and optional scrollback into a **new,
provably empty** owner mux after a reboot or server death. It cannot preserve prior process IDs —
what preserves live processes is ordinary GUI reattachment, not recovery.

Normal GUI restart never enters this path, because the server and its panes already exist.

## Why mux-startup is the only trigger

`mux-startup` runs exactly once for a new server, before the default program. That is the only
moment where "the server is empty" is a fact rather than a race.

It must never synchronously launch a child that reconnects to the same starting mux, and must never
query that mux through `wezterm cli` — both deadlock. All inspection is in-process
`wezterm.mux.all_windows()`.

If the registry-only lease helper cannot be proven deadlock-free, the service coordinator owns the
lease while Lua performs the in-process mux work.

## The algorithm

1. The service creates the fresh epoch and boot nonce before launch. `mux-startup` registers them,
   creates the reserved sentinel, and leaves the runtime descriptor `starting`.
2. Acquire the backend-instance kernel lock exclusively, plus its renewable fenced recovery lease.
   While held, every Wez create/rename/remove/adopt/repair/child mutation, write-like
   reconciliation, snapshot publication, and other recovery attempt is excluded. Reads observe the
   descriptor and return `recovering` plus the generation; no default user pane may spawn.
3. Inspect the tree in-process. Require exactly the valid sentinel and zero user panes.
4. Load the newest complete manifest for this backend instance whose registry revision is newer
   than that instance's `intentional_empty_revision`.
5. Create a durable recovery generation and journal keyed by server epoch, manifest ID, SpaceUid,
   and manifest node path. Exclude deleted, aborted, conflicted, unmanaged, and unhealthy Spaces.
6. Re-check the in-process tree and the lease fence immediately before the first restore and before
   each native-ID-dependent step.
7. For each manifest node, allocate or reuse its provisional bootstrap request and reconcile
   journal state against the native tree and token title.
8. Restore eligible Spaces in-process with their existing SpaceUid and opaque key, recording
   `preparing | restoring | completed | failed` transitions and node postconditions. Do not switch
   GUI presentation while restoring.
9. Verify the final one-window tree, reconcile epoch-qualified live handles, mark the descriptor
   `ready`, complete the generation, and release the lease.

A second starter that observes a completed generation does nothing.

If recovery is ineligible or no manifest qualifies, the sentinel remains the only pane and readiness
still succeeds. That is a normal outcome, not a failure.

## Descriptor and generation states

The service writes a mode-0600 descriptor beneath the runtime directory carrying `starting | ready
| failed`, PID and process start token, socket device and inode, backend-instance UID, epoch, and
boot nonce.

Generation node states are `preparing | restoring | completed | failed`.

An unrecoverable manifest or restore error marks the descriptor and generation `failed`, retains the
sentinel and journal, and keeps ordinary mutations blocked until `dmux doctor` directs an explicit
fenced resume or abort.

## Manifest eligibility

- Newer than the instance's `intentional_empty_revision`
- Complete, for this backend instance
- Excludes deleted, aborted, conflicted, unmanaged, and unhealthy Spaces

`intentional_empty_revision` is recorded when the final active user Space is removed, and only
after a complete same-epoch scan proves zero user panes. It is per backend instance. A later mux
startup never restores a manifest at or before that revision — this is what makes "I deliberately
cleared everything" stick across a reboot.

Owner-domain filtering matters: an imported remote pane must never be rewritten into a local shell.

## Crash, resume, and abort

A crash leaves the recovery lease and journal observable. Takeover follows the fencing rules in
plan §10.2 and **resumes the same generation**; it does not start a new blind restore.

A resumed generation either completes a node proven to have been created by that generation, or
safely removes and replaces only a recovery-created bootstrap partial. It never guesses by list
ordinal and never duplicates a Space, Group, or Split.

Crash-at-every-phase is a required test case, including a crash after native pane creation but
before the bootstrap or journal commit. Orphans are reconciled by reserved title and token under
the journal's opaque Space, parent, and manifest-node path: exactly one conforming orphan is
rebound, zero is retried only after confirmed absence, and multiple or ambiguous children become
`conflict`. An orphan with no valid journal never executes user code.

Abort may remove only nodes proven to belong to that recovery generation. It never deletes
pre-existing native state.

## Snapshot exclusion

Snapshot publication takes a mutually exclusive snapshot lease, so a manifest cannot capture a
half-restored or concurrently mutating tree. Snapshot and recovery are both database state scopes
over the same backend-instance exclusive lock — they are not separate native-exclusion locks.
