# ADR 0003: Warning code and detail mapping

Status: Accepted

Date: 2026-08-16

## Context

Diagnostic codes are versioned by stage and cannot grow without revising their owning version.
Source-v1 linting still needs stable machine-readable identities for every warning in section 24.2
of the language specification. A separate `lint/` code namespace would duplicate analysis and violate
the frozen code registry.

## Decision

Warnings use the existing stage-owned diagnostic code plus a required structured `detail` string.
The source-v1 warning mapping is:

| Condition | Stage | Code | Detail |
|---|---|---|---|
| Package directory without `package.dotfile` | `discovery` | `discovery/inventory` | `package_without_metadata` |
| New entity unusually close to another known name | `resolve` | `resolve/identity` | `near_entity_name` |
| Entity facts unused by every declared profile | `resolve` | `resolve/identity` | `unused_entity_facts` |
| Deployable facet empty after exclusions | `discovery` | `discovery/inventory` | `empty_deployable_facet` |
| Lexical binding has no use | `schema` | `schema/binding` | `unused_binding` |
| Node or facet departed relative to the previous lock | `lock` | `lock/stale` | `departed_identity` |

These six diagnostics have intrinsic severity Warning. Warning promotion is caller policy and does
not change severity, code, detail, semantic validity, IR, or lock bytes.

Every warning contains evidence appropriate to its condition. Near-name data includes the new
identity, candidates, and distance basis. Unused-fact data includes all contribution spans and the
validated profile set. Empty-facet data includes exclusions and inventory origins. Unused-binding
data includes its definition span and scope. Departure data names the previous qualified identity
and prior lock provenance.

The departed-identity warning is available only when a strict, previously valid lock is supplied as
comparison input. Its absence cannot change compilation results. It is not emitted when the prior
lock is noncanonical, tampered, or unsupported.

`dotfile lint` is the planned read-only command for this warning view. It reuses the shared analysis
database and common human and JSON diagnostic schema. `--deny-warnings` makes the caller exit
unsuccessfully but does not transform warnings into errors.

New warning conditions require a revision of the owning source, lock, or built-ins version before
their detail can enter that version's registry. Free-form and unregistered detail strings are
invalid.

## Consequences

Clients can suppress, promote, document, and test a warning without depending on prose. The stable
code list remains small, while details distinguish conditions within the responsible stage.

## Verification

Registry tests require every warning detail to map to exactly one existing code, stage, severity,
version owner, and normative condition. Golden diagnostic JSON covers positive, negative,
multi-origin, and ordering cases for every row.

Policy tests prove that default warnings exit successfully, `--deny-warnings` exits with the
diagnosed-result code, and neither policy changes emitted IR or lock bytes.
