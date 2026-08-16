# ADR 0007: Symbolic variant analysis

Status: Accepted

Date: 2026-08-16

## Context

The compiler must validate collisions for every satisfiable group-variant combination in every
declared profile. Eager Cartesian expansion is exponential. Version 1 permits at most one selected
variant or `none` per active group, and each deployment candidate's activation condition remains a
product of independent finite group domains.

## Decision

Variant activation is represented by finite-domain activation cubes.

For each active group `g` that declares variants, its domain `D(g)` is the explicit selection
`none` plus every declared variant name, sorted by unsigned UTF-8 bytes. A group without variants is
absent from the symbolic domain. Exactly one member of `D(g)` is selected in a bound plan.

A cube is a map from group identity to a non-empty allowed-selection set. An omitted group means
the complete domain `D(g)`. Stored group keys and selection values use byte order. Empty allowed
sets are never stored because they denote an unsatisfiable cube.

Candidate cubes are constructed as follows:

- a variant-only new leaf restricts its group to that variant;
- a variant replacement leaf restricts its group to that variant;
- a base leaf permits `none` and every variant that does not replace that logical leaf;
- a base leaf replaced by every possible selection is omitted as unreachable;
- selections of unrelated groups remain unrestricted.

The conjunction of two candidate predicates is cube intersection. For every group appearing in
either cube, intersect its allowed set with the other cube's explicit set or full domain. The result
is satisfiable exactly when every intersection is non-empty. This operation does not enumerate the
Cartesian product.

When an intersection is satisfiable, its canonical witness is the byte-lexicographically smallest
complete selection vector. Groups are visited by qualified group-ID bytes. For each group, choose
the smallest allowed selection bytes. The diagnostic records this complete witness so a collision
can be reproduced at bind time.

Collision analysis considers candidate pairs whose symbolic destinations are equal, prefix-related,
case-equivalent, or Unicode-equivalent under the profile rules. For a satisfiable pair, the compiler
materializes only the canonical witness and applies the normative variant replacement,
operational-deduplication, action-conflict, and cross-group overlay rules. Same-action cross-group
overlay is not reported as a collision; different actions and destination-prefix conflicts remain
errors. The witness affects diagnostics only and is never serialized as machine state.

Every profile is analyzed independently with only its active groups. The compile-time domain always
includes `none`; the runtime requirement for an explicit saved or invocation selection is a bind
precondition and does not remove `none` from compile-time validation.

Cube construction, intersection, witness selection, and candidate traversal are deterministic and
independent of filesystem, hash-map, or parallel evaluation order. Finite-domain cubes are the
version-1 representation; a more expressive future variant language requires a new decision and
source-version analysis.

## Consequences

The compiler validates combinations across groups without eagerly building their Cartesian
product. Satisfiability and witness construction are linear in the number of constrained group
domains for a candidate pair.

This representation depends on the version-1 invariant that each candidate activation predicate is
a product of per-group allowed sets. Arbitrary Boolean conditions, per-facet selections, and
whiteouts are outside this decision.

## Verification

Unit tests cover domain construction, base fallback, same-path replacement, variant additions,
`none`, empty intersections, unrestricted groups, and canonical witnesses containing non-ASCII
byte order edges.

Property tests generate small profiles with up to five variant-bearing groups and four variants per
group. They compare symbolic results and witnesses with exhaustive enumeration for exact,
same-group, cross-group, cross-action, prefix, case, and Unicode collision rules. Additional tests
shuffle every input collection and compare serial and parallel diagnostics byte for byte.

