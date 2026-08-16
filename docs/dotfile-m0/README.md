# `.dotfile` M0 contract freeze

M0 freezes the interfaces that later language, editor, lock, discovery, and runtime work must
implement. The normative language remains `docs/dotfile-language.md`; these records make its
implementation decisions and verification ownership executable.

## Scope

M0 provides:

- accepted decisions for parser recovery, spans, diagnostics, CLI behavior, generated ownership,
  version support, symbolic variant analysis, and guarded filesystem operations;
- machine-readable version, schema, diagnostic, producer, protocol, release, performance, fixture,
  and traceability contracts;
- a versioned conformance-fixture record format and manifest with later fixture cases planned;
- one test-support crate that validates contracts and models fake filesystem semantics;
- a platform feasibility result that later apply milestones must obey.

## Non-goals

M0 does not provide a complete lexer, parser, formatter, resolver, lock compiler, LSP server,
renderer, binder, observer, or apply engine. It does not enable destination mutation, claim
language or runtime conformance, publish release artifacts, or cut over the repository command.
It does not provide a completed conformance corpus.

The current repository sources are legacy inputs and remain untouched. They must not be silently
interpreted as source v1. Later development will use isolated source-v1 fixtures until the reviewed
migration and cutover milestones.

`dotfile lint` is registered as a planned source-v1 read-only command. It will expose warnings from
the shared analysis engine, accept caller policy such as `--deny-warnings`, and use the common
human and JSON diagnostic contracts. It will never bind, observe, apply, mutate machine
destinations, or become a second validation implementation.

## Accepted decisions

- [ADR 0001: Parser recovery](adrs/0001-parser-recovery.md)
- [ADR 0002: Span and offset representation](adrs/0002-span-and-offset-representation.md)
- [ADR 0003: Warning code and detail mapping](adrs/0003-warning-code-detail-mapping.md)
- [ADR 0004: CLI output and exit contract](adrs/0004-cli-output-and-exit-contract.md)
- [ADR 0005: Generated ownership](adrs/0005-generated-ownership.md)
- [ADR 0006: Supported version windows](adrs/0006-supported-version-windows.md)
- [ADR 0007: Symbolic variant analysis](adrs/0007-symbolic-variant-analysis.md)
- [ADR 0008: Guarded replace and prune](adrs/0008-guarded-operations.md)

## Contract artifacts

- [`versions.json`](../../contracts/dotfile/v1/versions.json) freezes the source, lock, built-ins,
  and supported reader and writer windows.
- [`schemas.json`](../../contracts/dotfile/v1/schemas.json) inventories domains,
  contexts, fields, types, cardinality, namespaces, and canonical ordering.
- [`diagnostics.json`](../../contracts/dotfile/v1/diagnostics.json) owns stages, stable codes,
  structured details, severities, and warning mappings.
- [`cli.json`](../../contracts/dotfile/v1/cli.json) freezes output framing, diagnostic transport,
  process exits, and stream separation.
- [`renderer-registry.json`](../../contracts/dotfile/v1/renderer-registry.json) assigns every native
  generated output to its producer and qualified facet.
- [`benchmark-producer.json`](../../contracts/dotfile/v1/benchmark-producer.json) freezes epoch,
  run-ID, validation, ordering, and atomic-update behavior.
- [`release.json`](../../contracts/dotfile/v1/release.json) records the LSP 3.17 floor, supported
  release targets, platform floors, product contents, signing, installation, and blocking status.
- [`performance.json`](../../contracts/dotfile/v1/performance.json) records corpora, operations,
  targets, measurement rules, reference hosts, and gate policy.
- [`apply-capabilities.json`](../../contracts/dotfile/v1/apply-capabilities.json) records no-replace
  creation and guarded replacement and prune support by platform and filesystem.
- [`fixtures.json`](../../contracts/dotfile/v1/fixtures.json) freezes the fixture record format and
  planned case inventory, including simulated state, expected outputs, and normative references.
- [`traceability.json`](../../contracts/dotfile/v1/traceability.json) assigns every normative
  `MUST` or `MUST NOT` occurrence to an owner, stage, milestone, and test plan.

## Contract support crate

[`dotfile-test-support`](../../scripts/rust/crates/dotfile-test-support) validates the M0 contract
artifacts and supplies deterministic fake filesystem semantics. It is test-only.

## Completion criteria

M0 is complete when:

1. all eight decisions are accepted and contain no unresolved alternative;
2. every contract artifact validates and all cross-references resolve;
3. every normative `MUST` or `MUST NOT` occurrence has an owner and a nonempty test plan;
4. every stable diagnostic code and warning detail has exactly one version owner;
5. every registered renderer output has one producer and one qualified facet;
6. benchmark epoch, run-ID, ordering, and mismatch rejection match reviewed golden vectors;
7. the LSP, artifact, platform, and performance matrices name exact supported configurations;
8. guarded replacement and prune fail closed on platforms without token-conditional namespace
   operations;
9. current legacy sources and the Python command remain operational and unmodified.
