//! Syntax descriptors for the script family (`Family::Curly`, `Engine::Script`).
//!
//! JavaScript, its module and JSX spellings, and TypeScript share the C-family
//! comment pair. Everything else the old `js.rs` scanner knows — regex versus
//! division, template `${…}` nesting, JSX markup, TypeScript type arguments,
//! Flow casts, HTML `<!-- -->` comments — is engine state, not descriptor data,
//! so it is deliberately absent here; the engine reads these markers and quotes
//! and keeps that state itself.
//!
//! `Quote` cannot express template interpolation: the `` ` `` entry describes
//! the delimiter, escape and multiline rule, but `js.rs` owns the `${…}` holes.
//! The same limit applies to JSX/TSX markup, which is parsed by the engine and
//! contributes no additional comment form.

use crate::syntax::*;

// The list carries every JS/TS ecosystem marker that `keep.rs` currently keeps
// globally, so the per-language table can take them over without losing a
// directive. `keep.rs` lowercases the comment body before matching, so markers
// are spelled lowercase even where the source spelling has capitals
// (`webpackChunkName`, `@__NO_SIDE_EFFECTS__`). `/*#__PURE__*/` is listed as
// written, but the marker that actually matches is `#__pure__` because
// `keep.rs` trims the block delimiters and any leading `*` from the body.
const SCRIPT_DIRECTIVES: &[&str] = &[
    // Linters and formatters.
    "eslint",
    "biome-ignore",
    "prettier-ignore",
    "deno-lint-ignore",
    "oxlint-",
    "rome-ignore",
    "jshint",
    "jslint",
    // TypeScript and Flow pragmas.
    "@ts-",
    "@ts-expect-error",
    "@ts-ignore",
    "@flow",
    "@noflow",
    "@jsx",
    // Coverage, bundler and test-runner hints.
    "istanbul ignore",
    "c8 ignore",
    "v8 ignore",
    "node:coverage",
    "webpackchunkname",
    "webpackignore",
    "turbopackignore",
    "webpackmode",
    "webpackprefetch",
    "webpackpreload",
    "webpackinclude",
    "webpackexclude",
    "@vite-ignore",
    "@jest-environment",
    "@vitest-environment",
    // Pure/optimizer annotations.
    "@__pure__",
    "#__pure__",
    "@__no_side_effects__",
    "#__no_side_effects__",
    "@__inline__",
    "@__noinline__",
    "@__key__",
    "@__mangle_prop__",
    "/*#__pure__*/",
    // Per-file lint/compiler pseudo-directives.
    "global ",
    "globals ",
    "exported ",
    "<reference",
    "<amd-",
    "#region",
    "#endregion",
    "@cc_on",
    "@generated",
];

const SCRIPT_LINE: &[LineComment] = &[LineComment {
    marker: "//",
    at: At::Anywhere,
    continues_on_backslash: false,
    doc: false,
    owns_indent_block: false,
}];

const SCRIPT_BLOCK: &[BlockComment] = &[BlockComment {
    open: "/*",
    close: "*/",
    nested: false,
    at: At::Anywhere,
    important: Some('!'),
    to_eof_ok: false,
}];

const SCRIPT_QUOTES: &[Quote] = &[
    // "…" — backslash escapes, no raw line break (a `\` + newline continues).
    Quote {
        open: "\"",
        close: "",
        escape: Escape::Backslash,
        multiline: false,
        prefixes: &[],
        hashes: false,
        raw_tag: false,
        text: Some(TextGlyphs {
            escapes: true,
            folds: true,
            delim: b'"',
        }),
    },
    // '…' — identical to `"…"` for comment scanning.
    Quote {
        open: "'",
        close: "",
        escape: Escape::Backslash,
        multiline: false,
        prefixes: &[],
        hashes: false,
        raw_tag: false,
        text: Some(TextGlyphs {
            escapes: true,
            folds: true,
            delim: b'\'',
        }),
    },
    // `…` — multiline template literal. `${…}` is engine state; see the module
    // note. `js.rs` records each non-hole segment as glyph-bearing text.
    Quote {
        open: "`",
        close: "",
        escape: Escape::Backslash,
        multiline: true,
        prefixes: &[],
        hashes: false,
        raw_tag: false,
        text: Some(TextGlyphs {
            escapes: true,
            folds: true,
            delim: b'`',
        }),
    },
];

/// JavaScript: classic scripts and CommonJS (`js`/`cjs`).
/// Engine note: `js.rs::html_comments()` also accepts `<!--`/`-->`, but only
/// for `Dialect::JavaScript`; `Syntax` has no field for that, so the engine
/// must keep the dialect check.
pub static JAVASCRIPT: Syntax = Syntax {
    name: "javascript",
    extensions: &["js", "cjs"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Script,
    line: SCRIPT_LINE,
    block: SCRIPT_BLOCK,
    quotes: SCRIPT_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: SCRIPT_DIRECTIVES,
    regions: &[],
};

/// ES modules. Comment and quote syntax is identical to JavaScript; the
/// separate descriptor exists so `Dialect::Mjs` resolves to its own name.
pub static MJS: Syntax = Syntax {
    name: "mjs",
    extensions: &["mjs"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Script,
    line: SCRIPT_LINE,
    block: SCRIPT_BLOCK,
    quotes: SCRIPT_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: SCRIPT_DIRECTIVES,
    regions: &[],
};

/// TypeScript: same comments as JavaScript. Type arguments and type assertions
/// are `js.rs` state (`typescript()`, `starts_type_arguments`), not table data.
pub static TYPESCRIPT: Syntax = Syntax {
    name: "typescript",
    extensions: &["ts", "mts", "cts"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Script,
    line: SCRIPT_LINE,
    block: SCRIPT_BLOCK,
    quotes: SCRIPT_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: SCRIPT_DIRECTIVES,
    regions: &[],
};

/// JSX: comments and strings are JavaScript's; markup is the script engine's
/// element state (`markup()`), so no descriptor field describes it.
pub static JSX: Syntax = Syntax {
    name: "jsx",
    extensions: &["jsx"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Script,
    line: SCRIPT_LINE,
    block: SCRIPT_BLOCK,
    quotes: SCRIPT_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: SCRIPT_DIRECTIVES,
    regions: &[],
};

/// TSX: TypeScript plus JSX. Both behaviours come from `js.rs`; the descriptor
/// only supplies the shared comment and quote tables.
pub static TSX: Syntax = Syntax {
    name: "tsx",
    extensions: &["tsx"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Script,
    line: SCRIPT_LINE,
    block: SCRIPT_BLOCK,
    quotes: SCRIPT_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: SCRIPT_DIRECTIVES,
    regions: &[],
};

/// Script-family descriptors, in registry order.
pub static SCRIPT: &[(&str, &Syntax)] = &[
    ("javascript", &JAVASCRIPT),
    ("mjs", &MJS),
    ("typescript", &TYPESCRIPT),
    ("jsx", &JSX),
    ("tsx", &TSX),
];
