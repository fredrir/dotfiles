# ADR 0006: Supported version windows

Status: Accepted

Date: 2026-08-16

## Context

Authored syntax, generated lock layout, and built-in semantic and runtime rules evolve separately
but are selected as one source-version tuple. Guessing, partial reads, or broad compatibility ranges
would make two consumers interpret the same repository differently.

## Decision

The only supported tuple is:

| Component | Read minimum | Read maximum | Write version |
|---|---:|---:|---:|
| Authored `.dotfile` | `1` | `1` | `1` |
| Generated lock | `1` | `1` | `1` |
| Built-ins | `1` | `1` | `1` |

Source version `1` selects exactly generated-lock version `1` and built-ins version `1`. The
compiler cannot select a different lock or built-ins version through a flag, environment variable,
configuration file, installed-tool probe, or previous lock. Version identifiers are opaque strings,
not quantities.

The bootstrap reader recognizes only one optional UTF-8 BOM at byte offset zero followed by the
exact ASCII source declaration as the first non-comment entry of `config/profiles.dotfile`.
Unsupported, absent, duplicate, interpolated, bound, or otherwise malformed declarations fail
without invoking a legacy parser. An unsupported source identifier uses `schema/context` with
detail `unsupported_dotfile_version` after the bootstrap range has been identified.

A strict lock reader checks source, lock, and built-ins identifiers before reading semantic
records. An unsupported lock identifier uses `lock/tampered` with detail
`unsupported_lock_version`. An unsupported built-ins identifier uses `lock/tampered` with detail
`unsupported_builtins_version`. A tuple inconsistent with the registered source selection uses
`lock/tampered` with detail `version_tuple_mismatch`. No consumer processes supported sections of
an otherwise unsupported lock.

The compiler, formatter, linter, source editor features, and native LSP support source version 1.
The lock writer emits only lock version 1. Lock-backed queries and runtime consumers accept only
lock version 1 with built-ins version 1. Destination mutation additionally recompiles source v1 and
byte-compares the complete canonical lock.

LSP 3.17 is a protocol floor rather than a language-version selector. Clients implementing newer
protocol revisions interoperate through advertised 3.17 capabilities; they do not enable newer
source, lock, or built-ins semantics.

Current repository sources remain legacy inputs until the explicit migration and cutover. Legacy
reading is available only through the planned `dotfile migrate --from legacy` workflow and never as
fallback after source-v1 bootstrap failure.

A future supported version requires a new explicit registry row, compatibility fixtures, updated
reader and writer windows, and an intentional tuple mapping. Ranges, wildcards, nearest-version
selection, and silent downgrade are forbidden.

## Consequences

Every source-v1 implementation emits and consumes the same semantic tuple. Unsupported repositories
fail early with a stable reason. Supporting a future version requires deliberate parallel reader
work rather than accidental forward compatibility.

## Verification

Bootstrap fixtures cover no BOM, one BOM, repeated and misplaced BOMs, missing and duplicate
declarations, every neighboring spelling, comments before the declaration, trailing commas,
bindings, interpolation, and unsupported identifiers.

Lock fixtures cover every supported and unsupported tuple combination, reordered or duplicate
version fields, partial lock content, and refusal before semantic record use. Product tests prove
that environment, flags, old locks, and tool release versions cannot alter tuple selection.

