# dotfmt-typescript

TypeScript, TSX, JavaScript and JSX formatting on top of the pinned
[`oxc_formatter`](https://github.com/oxc-project/oxc) release used by `oxfmt`
0.76. Defaults match Prettier 3.9: 80 columns, two spaces, double quotes,
semicolons, trailing commas everywhere and arrow parentheses.

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
- Plain CSS-in-JS tagged templates (`css`, `styled.*`, `styled(...)`, `createGlobalStyle`, `keyframes`) are formatted with the CSS engine when they contain no interpolation; interpolated templates are preserved.
- `prettier-ignore` is honored by the formatter. Formatting is idempotent and
  verifies with oxc's `detect_code_removal` so a layout bug cannot delete code.
- `sort_imports` uses oxc's Perfectionist-style grouping; `import_groups`
  accepts `builtin|external|internal|parent|sibling|index` (use `+` to combine
  entries), and `tsconfig` path aliases feed the internal prefixes.
- Tailwind class sorting supports `className`/`class`, custom functions such as
  `cn`/`clsx`/`cva`/`twMerge`, custom attributes and template literals.
- `analyze` reports unused imports, unused local variables, type-only imports,
  organize-imports edits, unused parameters/type parameters/class members and
  project-wide unused exports. Fixes are byte-range edits with
  `Safe`/`Unsafe`/`Suggestion` applicability; formatting never deletes code.
- `tsconfig.json`/`jsconfig.json` gates: classic JSX factory bindings
  (`React`/`h`), `verbatimModuleSyntax`, `importsNotUsedAsValues`,
  `preserveValueImports`, decorator metadata, `.d.ts`, `eval`/`new Function`.
- `type_aware = true` runs the project's `tsc --noEmit` for project diagnostics
  and builds an export/import graph for unused exports.
- Tests: `cargo test -p dotfmt-typescript`; benchmarks:
  `cargo bench -p dotfmt-typescript`; fuzzing: `fuzz/README.md`.
