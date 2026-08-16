# ADR 0008: Guarded replace and prune

Status: Accepted

Date: 2026-08-16

## Context

The runtime requires replacement and prune to be one linearizable namespace operation that
atomically compares the destination's recorded object token and mutates only on equality. A
userspace identity check followed by rename or unlink does not satisfy the contract. No-replace
creation solves only the distinct absent-destination case.

## Decision

The M0 capability result is:

| Platform and filesystem | Descriptor-relative no-follow traversal | No-replace create | Token-conditional replace | Token-conditional prune | Automatic replace or prune |
|---|---|---|---|---|---|
| Linux 5.15 or newer on ext4 | Required | Supported with object-specific exclusive creation or `renameat2` plus `RENAME_NOREPLACE` | Unsupported | Unsupported | Refused |
| Darwin on case-sensitive APFS | Required | Supported with object-specific exclusive creation or `renameatx_np` plus `RENAME_EXCL` | Unsupported | Unsupported | Refused |
| Darwin on case-insensitive APFS | Required | Supported with object-specific exclusive creation or `renameatx_np` plus `RENAME_EXCL` | Unsupported | Unsupported | Refused |
| Unregistered platform or filesystem | Required if inspection is possible | Unsupported until probed | Unsupported | Unsupported | Refused |

No-replace creation is a separate primitive. Regular materialized files are written to a unique
same-directory temporary and installed only by a kernel no-replace operation. Directory creation,
symlink creation, and other object-specific creation use APIs that atomically fail when the final
name exists. An existing or concurrently appearing destination produces a conflict and leaves that
destination unchanged.

Neither `renameat2`, `renameatx_np`, exchange-style rename, `unlinkat`, advisory locks, file
descriptors to the old object, nor a userspace check followed by mutation atomically predicates the
namespace operation on the required volume, file, and generation or change token. They therefore
do not implement guarded replacement or guarded prune.

Object tokens may still support observation and desired-state comparison. A matching token does not
authorize an unconditional rename or unlink. Replacement and prune of a recorded object are refused
before any destination write on ext4 and APFS, even when the object currently matches the ledger.

The runtime capability adapter probes the actual destination volume during complete preflight. A
missing, unknown, changed, or weaker capability fails closed. Capability results cannot be supplied
by repository source, environment variables, or an advisory lock.

When guarded replacement or prune is unavailable, the command reports the exact affected paths and
requires the separately confirmed backup/adopt workflow. That workflow is not implicit in `link`,
system installation, `--force`, or noninteractive approval. It must preserve the prior object before
recording a newly adopted identity.

The privileged helper enforces the same capability result independently. Elevation does not permit
an unconditional system overwrite or prune.

## Consequences

Initial apply support can safely create absent objects and report exact no-ops, but cannot
automatically update or remove existing managed objects on the named ext4 or APFS configurations.
This is an intentional fail-closed limitation rather than a weakened race guarantee.

Future platform support may change a matrix cell only after a reviewed primitive proves atomic
token comparison and mutation at one linearization point and passes the race suite on the named
filesystem.

## Verification

Capability tests run on ext4, case-sensitive APFS, and case-insensitive APFS. They prove no-replace
creation succeeds only for an absent name and loses safely to a concurrent creator.

Race injection replaces, removes, recreates, mutates, and changes metadata on a destination between
observation and execution. Replacement and prune must refuse without mutating the destination.
Tests also reject exchange-and-rollback, check-then-rename, check-then-unlink, advisory-lock, stale
token, unsupported-volume, and privileged-helper bypass implementations.

