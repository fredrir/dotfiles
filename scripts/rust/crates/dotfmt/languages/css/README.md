# dotfmt-css

CSS, SCSS, Sass and Less formatting via [`malva`](https://github.com/g-plane/malva),
including Tailwind v4 `@theme`/`@utility`/`@custom-variant`, nesting,
`@container`, `@property`, `@starting-style`, `@apply` class sorting and
`prettier-ignore`.

```rust
let output = dotfmt_css::format(
    Path::new("src/app.css"),
    input,
    &dotfmt_css::Config::default(),
    dotfmt_css::Dialect::Auto,
)?;
```

Defaults match Prettier CSS: 80 columns, two spaces, lowercase hex and double
quotes. `@apply` sorting uses the shared Tailwind sorter; declarations are never
reordered unless `declaration_order` is explicitly enabled. Syntax errors are
reported with line/column and leave the file untouched.

Tests: `cargo test -p dotfmt-css`; benchmarks: `cargo bench -p dotfmt-css`;
fuzzing: `fuzz/README.md`.
