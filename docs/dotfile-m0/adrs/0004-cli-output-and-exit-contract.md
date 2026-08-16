# ADR 0004: CLI output and exit contract

Status: Accepted

Date: 2026-08-16

## Context

Humans, hooks, CI, editor commands, and migration tooling need stable output behavior. Validation
failures, check-mode differences, invalid invocations, I/O failures, internal defects, and
interruptions must remain distinguishable without assigning new language diagnostic codes.

## Decision

Commands use human output by default and accept the global `--json` option. Human command results
are written to stdout. Human diagnostics and progress are written to stderr. ANSI styling is used
only when stderr is an interactive terminal and color has not been disabled.

JSON mode writes exactly one UTF-8 JSON document followed by one LF to stdout. It writes no
progress, banners, prompts, or ANSI sequences to stdout. The top-level object has these fields in
this presentation order:

1. `protocol_version`
2. `command`
3. `outcome`
4. `changed`
5. `diagnostics`
6. `data`

`protocol_version` is the string `"1"`. `command` is the normalized command path. `outcome` is one
of `success`, `difference`, `failure`, `interrupted`, or `internal_error`. `changed` is a Boolean,
`diagnostics` is an array in canonical diagnostic order, and `data` is always an object. Commands
do not omit these fields and do not emit JSON null for an absent result.

Each JSON diagnostic uses the shared registry and includes stage, severity, code, summary, remedy,
primary span, related spans, semantic identity, structured expected and actual data, provenance,
redaction state, scope, and an optional structured fix. It includes `detail` only when the registry
requires or permits one for that code; the six warning mappings always require it. Paths are
repository-relative unless a machine-scoped diagnostic requires a redacted symbolic destination.

The process exit codes are:

| Code | Meaning |
|---:|---|
| `0` | The requested operation succeeded under caller policy |
| `1` | A diagnosed validation, policy, check, drift, conflict, or consumer result was negative |
| `2` | The invocation, option combination, or command input was invalid |
| `3` | Repository I/O or required external transport failed |
| `70` | An internal invariant failed |
| `130` | The operation was interrupted or canceled by the user |

Exit code `1` includes source errors, `--deny-warnings`, `fmt --check` differences, `lock --check`
differences, failing checks, renderer drift, and apply precondition conflicts. Warnings without
promotion do not change exit code zero. Read-only commands that are defined to display stale state
may still exit zero; commands whose purpose is to verify freshness exit one.

Exit code `2` is emitted before semantic analysis. When `--json` can be recognized safely despite
the invocation error, the command emits the JSON envelope; otherwise the argument parser emits one
concise human error on stderr. Exit code `3` is not represented by a fabricated language diagnostic.
Its structured failure belongs in `data` with redacted operation and path information.

An I/O failure after successful analysis takes precedence over diagnosed warnings. An interruption
takes precedence over ordinary failures. An internal invariant takes precedence over every
completed result. Panics are contained at the product boundary where possible, reported as exit 70,
and never expose source secrets or private rendered bytes.

Machine-oriented package emission remains command data inside the JSON envelope in JSON mode. No
command switches to newline-delimited JSON under protocol version 1.

## Consequences

Callers can distinguish expected negative results from bad invocations, infrastructure failures,
and defects. Diagnostic versioning remains separate from transport and process failures.

Commands must buffer enough structured state to emit one complete JSON document and must avoid
library code that writes directly to process streams.

## Verification

Black-box tests cover every exit code, stdout and stderr separation, one-document JSON framing,
field presence, diagnostic ordering, color suppression, broken pipes, interrupts, denied warnings,
check-mode differences, redaction, and partial-write failures.

JSON schema tests reject unknown protocol versions, missing envelope fields, null result fields,
unregistered diagnostic details, absolute repository paths, and noncanonical severity spellings.
