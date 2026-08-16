# `.dotfile` M1 source and syntax

M1 delivers the language core's byte-level foundation: raw source handling, spans and line
indexes, the version bootstrap reader, the hand-written lexer, the lossless CST, the recovering
parser, and the generic AST. The normative basis is `docs/dotfile-language.md` sections 1, 4, and
6–8, ADR 0001 (parser recovery), ADR 0002 (span and offset representation), and ADR 0006
(supported version windows).

## Scope

M1 provides:

- [`dotfile-source`](../../scripts/rust/crates/dotfile-source): `FileId`, strict `RepoPath`,
  `SourceText`, checked `ByteRange`, the shared `LineIndex` (normative one-based Unicode-scalar
  coordinates, pseudo-scalar columns for malformed UTF-8, and LSP UTF-8/UTF-16 conversions), the
  frozen `Diagnostic` record with the capped `DiagnosticSink`, and the source version bootstrap
  reader for the exact `@dotfile-version = "1"` preamble;
- [`dotfile-syntax`](../../scripts/rust/crates/dotfile-syntax): the byte lexer (significant
  tokens plus `tokens + 1` trivia gaps, string/path side tables with decoded segments, escapes,
  and interpolation subspans), the deterministic recovering parser (event builder, ADR 0001 sync
  sets, missing tokens, depth/work budgets, diagnostic cap), the immutable lossless CST with
  exact byte replay, and the generic AST with contextual identifier/binding validation;
- 143 versioned conformance fixture records in
  [`contracts/dotfile/v1/fixtures`](../../contracts/dotfile/v1/fixtures) covering the bootstrap
  matrix and every section 6–8 case group with positive and negative neighbors, run by the
  `dotfile-test-support` fixture runner against golden token/trivia dumps, CST dumps, and
  diagnostic JSON;
- bounded fuzzing (arbitrary bytes, token soup, document mutations, adversarial nesting, escape
  and interpolation storms) asserting no panic, exact replay, determinism, and the diagnostic
  limit, plus span round-trip properties across byte, normative, UTF-8, and UTF-16 coordinates.

## Non-goals

M1 does not provide domain schemas, typed lowering, bindings and prologue rules, the formatter,
the semantic compiler, discovery, the lock, the CLI, tree-sitter, or the LSP. Domain meaning is
deliberately absent from the CST: `?@font` is valid generic syntax here, and contextual rules
such as the `@let` prologue or `@dotfile-version` placement beyond the bootstrap checks land in
M2. The legacy repository sources remain untouched and are still not interpreted as source v1.

## Implementation notes

- One authoritative parser serves every consumer; there is no editor-only dialect. Any lex or
  parse error prevents validated compiler IR downstream (`Parse::has_errors`).
- The CST owns trivia gaps directly. Replaying gaps and token byte slices reproduces the input
  exactly, including invalid and recovered regions; missing tokens contribute no bytes. Every
  fixture and fuzz input asserts replay equality.
- Recovery consumes an unexpected closer into the smallest error node (never closing multiple
  levels), inserts a missing closer at EOF or before a valid outer delimiter, recovers a
  newline-separated list value as one missing comma, and guarantees forward progress in every
  loop. Depth (256), work (`4096 + 64 * tokens`), and retained diagnostics (512, with the last
  retained diagnostic reporting the suppressed count) follow ADR 0001; exhaustion produces one
  lossless tail error node and one `parse/syntax` diagnostic with detail `resource_limit`.
- The bootstrap reader recognizes one optional BOM at byte zero and the exact ASCII declaration
  as the first non-comment entry of `config/profiles.dotfile`; unsupported, absent, duplicate,
  interpolated, bound, misplaced, or wrong-file declarations fail with `schema/context` or
  `schema/duplicate` diagnostics and never invoke a legacy parser (ADR 0006).
- String tokens carry a side table of decoded literal/interpolation segments and escape spans so
  later stages never reparse token text. Bare and quoted `PATHREF` forms are single tokens with
  decoded-path validation at lex time.

## Verification

- `cargo test -p dotfile-source -p dotfile-syntax -p dotfile-test-support` runs unit tests,
  golden fixtures, and bounded fuzzing.
- `cargo run -p dotfile-syntax --example dump -- lex|parse|bootstrap [--json] [--path p]` prints
  token/trivia dumps, CST dumps, and diagnostic JSON for fixture authoring and review.
- `contracts/dotfile/v1/fixtures.json` records the 143 implemented and passing fixture ids in
  `implementation_claims`; conformance is not claimed (`conformance_claimed: false`).
