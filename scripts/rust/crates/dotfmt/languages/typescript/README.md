# dotfmt-typescript

TypeScript, TSX, JavaScript and JSX formatting on top of the pinned
[`oxc_formatter`](https://github.com/oxc-project/oxc) release used by `oxfmt`
0.76. Defaults match Prettier 3.9: 80 columns, two spaces, double quotes,
semicolons, trailing commas everywhere and arrow parentheses.

dotfmt is a formatter, not a linter: diagnostics are errors only, and `analyze`
findings are reached through `dotfmt --diagnose`, `--fix` and LSP code actions.

```rust
let config = dotfmt_typescript::Config::default();
let output = dotfmt_typescript::format(
    Path::new("src/App.tsx"),
    input,
    &config,
    dotfmt_typescript::Dialect::Auto,
)?;
```

- BOM, CRLF/Windows line endings and `final_newline`; syntax failures are
  `DiagnosticKind::Syntax` with a line and column.
- Plain CSS-in-JS tagged templates (`css`, `styled.*`, `styled(...)`, `createGlobalStyle`, `keyframes`) are formatted with the CSS engine. `${...}` interpolations are masked during formatting and restored byte-for-byte.
- `expand_range` maps a byte selection to the smallest enclosing statement, JSX node, object/array literal, or program that is safe to format.
- `prettier-ignore` is honored by the formatter. Layout formatting is idempotent
  and verifies with oxc's `detect_code_removal` so a layout bug cannot delete
  code; only the configured fix options below remove code.
- `sort_imports` uses oxc's Perfectionist-style grouping; `import_groups`
  accepts `builtin|external|internal|parent|sibling|index` (use `+` to combine
  entries), and `tsconfig` path aliases feed the internal prefixes.
- Tailwind class sorting supports `className`/`class`, custom functions such as
  `cn`/`clsx`/`cva`/`twMerge`, custom attributes and template literals.
- `analyze` returns findings and byte-range fixes (`Safe`/`Unsafe`/`Suggestion`)
  for unused imports, locals, parameters, type parameters, class members,
  type-only imports, organize-import edits and unused exports.
  `Options::project_analysis` adds project-wide unused-export findings and
  unsafe fixes;
  `Options::type_aware` runs `tsc` and adds its errors. Editor diagnostics keep
  errors only and filter the unused-only codes 6133, 6138, 6192, 6196, 6198,
  6199, 6205, 7027 and 7028, so dotfmt publishes no lint findings, warnings or
  hints. Findings stay
  reachable through `dotfmt --diagnose`, `--fix`, `--fix-unsafe` and LSP code
  actions.
- `fixes = off|safe|unsafe` in `dotfmt.dotfile` selects format-time fixes;
  `safe` implies `remove_unused_imports`, `convert_type_only_imports` and
  `organize_imports`, `unsafe` adds `remove_unused_locals`. Export removal is
  never implied and must be requested explicitly with `remove_unused_exports`;
  unused locals with side-effecting initializers additionally need
  `remove_impure_locals`; parameter removal is diagnose-only because no safe
  fix exists. The matching booleans default to `false` and force the
  implemented categories on. Fixes run before the final format, at most 3
  passes, and stay idempotent; the defaults delete nothing.
- `tsconfig.json`/`jsconfig.json` gates: classic JSX factory bindings
  (`React`/`h`), `verbatimModuleSyntax`, `importsNotUsedAsValues`,
  `preserveValueImports`, decorator metadata, `.d.ts`/`.d.mts`/`.d.cts`,
  `eval`/`new Function`. Project export fixes also require a complete graph, a
  private package and an undecorated, comment-free deletion range.
- `type_aware = true` (default) runs the project's `tsc --noEmit` for type
  errors; unused exports need `project_analysis`.
- Tests: `cargo test -p dotfmt-typescript`; benchmarks:
  `cargo bench -p dotfmt-typescript`; fuzzing: `fuzz/README.md`.
