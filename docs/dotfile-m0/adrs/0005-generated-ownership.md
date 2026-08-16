# ADR 0005: Generated ownership

Status: Accepted

Date: 2026-08-16

## Context

The repository contains a compiler-owned lock, producer-owned benchmark baselines,
renderer-owned native theme outputs, and ordinary authored sources. A banner, filename, or path
shape alone cannot safely determine ownership. The CLI, formatter, LSP, rename engine, code actions,
and producers need one consistent classification.

## Decision

One central ownership index classifies a canonical repository-relative path using its validated
workspace role, typed domain, and registered renderer assignment. Classification occurs without
following symlinks. Basenames, banners, extensions, generated-looking contents, and inferred group
or package names are never sufficient.

An existing path must resolve to exactly one ownership role. Conflicting role claims, duplicate
renderer outputs, paths outside the workspace, noncanonical paths, and renderer outputs without a
qualified facet are contract errors.

The roles and permissions are:

| Role | Read and navigate | Diagnose | Source formatter | Direct rename or fix | Authorized writer |
|---|---|---|---|---|---|
| Generated lock | Yes | Strict schema, canon, tamper, freshness | Check only | Never | `dotfile lock` |
| Benchmark baseline | Yes | Benchmark schema | Yes | Syntax only | Benchmark baseline producer |
| Registered native theme output | Yes | Renderer drift | No | Never through `.dotfile` tools | Registered renderer |
| Authored source or override | Yes | Normal source diagnostics | Yes | Safe source actions | Human or explicit source command |

`package.lock.dotfile` is compiler-owned only when it occupies the exact workspace lock role and
passes strict generated-domain classification. The LSP may provide diagnostics, symbols, tokens,
hover, provenance, and source navigation. It may not offer completion, rename, direct fixes, or a
formatting edit. Lock regeneration is an explicit command using saved disk state and atomic
replacement after successful compilation.

`benchmarks/baselines.dotfile` is producer-owned semantic data. The normal formatter and proven
syntax-only fixes are allowed. Host, epoch, and run-ID insertion, removal, or rename is restricted
to the benchmark producer, which validates correlation and writes atomically.

A native theme output is renderer-owned only when `renderer-registry.json` contains its exact path,
producer, and qualified facet. The `.dotfile` LSP may link it to its renderer, facet, and theme
inputs and may report renderer drift. It may not format, rename, repair, generate, or stage the
artifact. A renderer check is read-only and scoped to the exact artifacts requested by lock or
destination planning.

An override source remains authored source unless another exact ownership record applies. The
presence of an `overrides` directory does not imply generated or read-only ownership.

Unsaved editor buffers may contribute to preview analysis, but no producer may claim that an
on-disk generated file is current for unsaved bytes. Lock regeneration, benchmark updates, and
renderer writes operate on saved repository state. Every authorized write uses validation,
same-directory temporary output where applicable, flush, and atomic installation.

## Consequences

Generated artifacts cannot be corrupted by generic formatting, rename, or fix-all operations.
Producer-specific navigation and diagnostics remain available. Adding a renderer output requires a
reviewed registry change rather than relying on a new path convention.

## Verification

Contract tests require unique normalized paths, known producer identities, qualified facet IDs,
and no overlap between lock, benchmark, renderer, and authored roles. A parity test compares the
registered theme inventory with the existing producer inventory during migration.

CLI and LSP tests exercise every allowed and refused operation for every role, including deceptive
banners, deceptive basenames, symlinks, unsaved buffers, stale lock input, renderer drift scoped to
one artifact, atomic producer writes, and generated-lock workspace edit refusal.

