# dotfmt-tailwind

Shared Tailwind class engine for TypeScript, CSS and HTML.

- Bracket/quote/escape-aware tokenizer and a stable sorter: unknown classes
  first, project order ascending, `...`/`…` last, duplicate known classes
  removed.
- `bridge::project_order` runs the project's own
  `prettier-plugin-tailwindcss` sorter (or Tailwind v4's design-system API)
  through a Node subprocess, batches only unseen classes and caches the order
  under `${XDG_CACHE_HOME:-~/.cache}/dotfmt/tailwind`.
- `OrderSource::Project` requires project order; `Auto` falls back to the Rust
  pattern sorter so a missing Node/Tailwind never blocks formatting.
- `Config.merge` is opt-in; it prefers `tailwind-merge` through Node, then a
  deliberately conservative Rust subset that only merges same-variant,
  same-family known classes.
- `extract::candidates` over-collects class strings to warm the order cache; it
  never decides where classes are sorted.

Tests: `cargo test -p dotfmt-tailwind`; benchmarks:
`cargo bench -p dotfmt-tailwind`.
