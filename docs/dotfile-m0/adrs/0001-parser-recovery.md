# ADR 0001: Parser recovery

Status: Accepted

Date: 2026-08-16

## Context

The CLI, formatter, compiler, linter, and LSP require one authoritative syntax tree. Editors also
need useful structure while a document is temporarily incomplete. Recovery must preserve every
input byte, terminate on adversarial input, avoid cascaded semantic errors, and never allow an
uncertain subtree into validated compiler IR.

## Decision

The authoritative parser is deterministic recursive descent with an event builder. It parses the
generic grammar without domain semantics and always returns an immutable CST. Parse failures are
represented by lossless error nodes and zero-width missing-token nodes.

An error node owns every consumed significant token and trivia gap in its range. A missing token is
anchored at `p..p`, where `p` is the byte offset at which the token was expected. CST replay uses
original token and gap byte slices and therefore reproduces the complete input, including invalid
and recovered regions. Missing tokens contribute no bytes.

Recovery uses these synchronization sets:

- a file or block body synchronizes at a comma, a newline run, its matching `}`, or EOF;
- a list synchronizes at a comma, `]`, or EOF;
- a newline-separated list value inserts one missing comma before resuming;
- EOF or a valid outer delimiter inserts a missing closer for the current construct;
- an unexpected closer is consumed into the smallest error node and never closes multiple levels;
- an unparsed remainder caused by exhausted work is one lossless tail error node.

Every parser loop must either consume at least one significant token or insert one missing token.
The same expected terminal may be inserted only once at the same byte offset in the same recovery
context. A second attempt consumes input into an error node or moves to the enclosing recovery
set.

Poison attaches to the smallest HIR value that consumes an error or missing node. Poison propagates
only to dependent fields and queries. Independent syntax and semantic inputs continue to produce
diagnostics. Any lex or parse error prevents sealed compiler IR and lock emission.

The implementation limits are:

- maximum parser nesting depth: 256;
- work budget for `n` significant tokens: `4096 + 64 * n` units;
- retained lexer and parser diagnostics per file: 512;
- diagnostics published to an LSP client per document: 100 after canonical sorting.

One unit is charged for a token inspection, token consumption, event emission, missing-token
insertion, or recovery transition. Arithmetic is checked. When depth or work is exhausted, parsing
stops descending, consumes the affected remainder into one error node, and emits one
`parse/syntax` diagnostic with detail `resource_limit`. Reaching an implementation limit is a
failed analysis request, not a new source-v1 grammar rule, and implementations may raise these
limits without changing the language version.

When the retained-diagnostic limit is reached, parsing continues within the work budget but
additional diagnostics are counted and suppressed. The last retained diagnostic reports the
suppressed count in structured data. The LSP publication cap never changes CLI results, compiler
validity, or diagnostic ordering.

## Consequences

The CST is useful during incomplete edits while strict compilation remains unchanged. Recovery is
bounded and deterministic, and semantic consumers can distinguish known values from poisoned
values without maintaining an editor-only dialect.

The parser must maintain explicit recovery state and budgets. A resource-limit result cannot be
cached as proof that source is invalid under version 1.

## Verification

Golden tests cover every production and its immediate malformed neighbors, exact CST replay,
missing-token anchors, unexpected closers, nested poison, and deterministic diagnostic order.

Property and fuzz tests assert forward progress, bounded work, bounded stack depth, no panic,
identical output across repeated runs, and no valid IR when any lex or parse error exists. Separate
goldens fix EOF, BOM, CRLF, astral, combining-character, invalid UTF-8, and tail-error behavior.

