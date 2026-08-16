# `.dotfile` M2 typed domains and formatter

M2 adds the path-selected language layer above M1's lossless syntax tree. It classifies every
registered source path, lowers authored files into owned typed HIR with raw-byte provenance,
validates immediate domain rules, and formats parse-valid `.dotfile` source canonically. The
normative basis is `docs/dotfile-language.md` sections 4–9, 12–14, 23, and 26, together with the
versioned schema and diagnostic contracts under `contracts/dotfile/v1`.

## Scope

M2 provides:

- `dotfile-schema`: the eighteen-domain classifier, validated dynamic group layout, the shared v1
  schema and formatting registry, tolerant owned HIR, lexical binding scopes, segment provenance,
  and a bidirectional `HirId`/raw-byte source map;
- immediate typed validation for profiles, hosts, group-root requirements, facets, override
  variants, recipient keys, secret-scan rules, and benchmark baselines;
- `dotfile-theme`: owned, source-ordered models and immediate validation for shared roles, fonts,
  theme profiles, and all five registered application maps;
- `dotfile-format`: whole-document canonical formatting over the M1 CST and the shared
  `FormatSchema`, including comment attachment, stable domain ordering, canonical strings and
  paths, four-space layout, Unicode-scalar width, and idempotence checks;
- M2 conformance fixtures and property tests for schema contexts, bindings, source maps, themes,
  formatter preservation, comment movement, and generated-file refusal.

Classification is based only on canonical repository-relative paths and an explicitly supplied,
validated group-directory layout. It never matches file contents, follows symlinks, reads ambient
machine state, or walks the repository. `config/profiles.dotfile` supplies the dynamic layout in a
later repository analysis pipeline; M2 exposes the pure types and operations needed for that
two-phase process.

## Boundaries

The HIR is owned rather than self-referential: it copies semantic text and retains opaque syntax
node identities plus checked `ByteRange` anchors in a separate source map. Syntax node identities
are never serialized or used as stable semantic IDs. A validated result is sealed only when lex,
parse, and schema errors are absent; tolerant HIR and independent diagnostics remain available for
editor use.

Bindings are file-local lexical string macros. Each file and block has a source-ordered prologue,
inner scopes may shadow outer definitions, and same-scope redeclaration, self-reference,
use-before-declaration, and declarations after the prologue are schema errors. Definition,
reference, interpolation-segment, and evaluated-value provenance remains available through the
source map.

The formatter refuses lex/parse-invalid source, the compiler-owned generated lock, and the
encrypted YAML variable store. It remains total over parse-valid schema-invalid source and
preserves unknown entries, duplicates, keyless resources, ordered lists/maps/records, and their
stable relative order. Formatting never resolves names, expands a profile, discovers payload, or
mutates a repository or machine destination.

## Non-goals

M2 does not implement cross-file namespace resolution, profile expansion, theme merging, palette
or map joins, demand occurrences, fact merging, checks/adapters, or graph analysis; those are M3.
M3 also owns the theme-inventory validation in DFV1-MUST-065: mandatory-file cardinality and the
tracked, ordinary-file, non-symlink requirements. M4 supplies that validator with hermetic Git and
filesystem snapshot facts as part of general repository discovery, deployment inheritance,
variants, collisions, compiler IR, and generated-lock work. CLI commands, LSP integration,
tree-sitter, binding to a machine, and all destination mutation remain later milestones. The
current legacy repository sources and Python command are not migrated or reinterpreted as v1.

## Verification

Run the focused M2 suite from `scripts/rust`:

```bash
cargo test -p dotfile-source -p dotfile-syntax -p dotfile-schema \
  -p dotfile-theme -p dotfile-format -p dotfile-test-support
cargo clippy -p dotfile-source -p dotfile-syntax -p dotfile-schema \
  -p dotfile-theme -p dotfile-format -p dotfile-test-support \
  --all-targets -- -D warnings
cargo fmt --all -- --check
```

The full workspace remains available through `cargo test --workspace`. Legacy behavior is checked
from the repository root with `tests/run.sh` and the existing Python suite.
