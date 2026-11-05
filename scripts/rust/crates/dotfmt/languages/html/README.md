# dotfmt-html

HTML, Vue, Svelte, Astro, Angular and XML formatting via
[`markup_fmt`](https://github.com/g-plane/markup_fmt). Embedded `<style>` and
`<script>` blocks are formatted with `dotfmt-css` and `dotfmt-typescript`; an
invalid embedded snippet keeps its original text instead of failing the file.

Static `class`/`className` (and configurable) attributes are sorted with the
shared Tailwind sorter. Dynamic Vue `:class` object/array expressions and
template interpolations are left alone. `prettier-ignore` and
`prettier-ignore-file` are supported.

Tests: `cargo test -p dotfmt-html`.
