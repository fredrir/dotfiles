//! Syntax descriptors for every stylesheet (`Family::Css`, `Engine::Style`).
//!
//! All six dialects share a non-nesting `/* */` block comment and the two
//! backslash-escaped string forms. Only plain CSS has no line comment, and only
//! Sass treats a line comment at the start of a line as owning the indented
//! block beneath it.
//!
//! `Style` owns everything the table cannot say: SCSS/Sass `#{…}` interpolation,
//! Less `@{…}` interpolation and backtick JavaScript, Stylus `@css { … }`
//! literals, custom-property bodies (`--x: …`), and `url(…)` scanning. Plain
//! `/* */` nesting is not a CSS form; none of the six nests.

use crate::syntax::*;

const STYLE_DIRECTIVES: &[&str] = &[
    "stylelint",
    "autoprefixer",
    "purgecss",
    "clean-css",
    "postcss",
    "tailwindcss",
    "cssnano",
    "uncss",
    "rtl:",
    "noinspection",
    "sass-lint",
    "scss-lint",
    "less-lint",
    "stylus-lint",
];

const STYLE_BLOCK: &[BlockComment] = &[BlockComment {
    open: "/*",
    close: "*/",
    nested: false,
    at: At::Anywhere,
    important: Some('!'),
    to_eof_ok: false,
}];

const STYLE_QUOTES: &[Quote] = &[
    // `css.rs::quoted` permits `\` + newline as a continuation but rejects a raw
    // line break, hence `multiline: false`.
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
];

/// The `//` line comment shared by every dialect except plain CSS.
const SLASH_LINE: &[LineComment] = &[LineComment {
    marker: "//",
    at: At::Anywhere,
    continues_on_backslash: false,
    doc: false,
    owns_indent_block: false,
}];

/// Plain CSS: block comments only. `//` is not a comment (`url(//host/x)`).
pub static CSS: Syntax = Syntax {
    name: "css",
    extensions: &["css", "pcss", "postcss"],
    filenames: &[],
    shebangs: &[],
    family: Family::Css,
    engine: Engine::Style,
    line: &[],
    block: STYLE_BLOCK,
    quotes: STYLE_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: STYLE_DIRECTIVES,
    regions: &[],
};

/// SCSS: `//` plus `/* */`; `#{…}` interpolation is engine state.
pub static SCSS: Syntax = Syntax {
    name: "scss",
    extensions: &["scss"],
    filenames: &[],
    shebangs: &[],
    family: Family::Css,
    engine: Engine::Style,
    line: SLASH_LINE,
    block: STYLE_BLOCK,
    quotes: STYLE_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: STYLE_DIRECTIVES,
    regions: &[],
};

/// Sass (indented syntax): a `//` at the start of a line silently comments out
/// its indented block, so the line marker owns that block. Engine note: the
/// same is true of a loud `/* …` at line start in `css.rs::indented_block`, but
/// `BlockComment` has no `owns_indent_block` field; `Style` must keep that rule.
pub static SASS: Syntax = Syntax {
    name: "sass",
    extensions: &["sass"],
    filenames: &[],
    shebangs: &[],
    family: Family::Css,
    engine: Engine::Style,
    line: &[LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
        owns_indent_block: true,
    }],
    block: STYLE_BLOCK,
    quotes: STYLE_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: STYLE_DIRECTIVES,
    regions: &[],
};

/// Less: `//` and `/* */`; `@{…}` interpolation and backtick JavaScript are
/// `css.rs::interpolation` / `less_js`, neither of which a `Quote` can express.
pub static LESS: Syntax = Syntax {
    name: "less",
    extensions: &["less"],
    filenames: &[],
    shebangs: &[],
    family: Family::Css,
    engine: Engine::Style,
    line: SLASH_LINE,
    block: STYLE_BLOCK,
    quotes: STYLE_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: STYLE_DIRECTIVES,
    regions: &[],
};

/// Stylus: an unterminated block comment legitimately runs to EOF
/// (`css.rs::block_to_eof`), so `to_eof_ok` is true. `@css { … }` literals are
/// engine state.
pub static STYLUS: Syntax = Syntax {
    name: "stylus",
    extensions: &["styl"],
    filenames: &[],
    shebangs: &[],
    family: Family::Css,
    engine: Engine::Style,
    line: SLASH_LINE,
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: Some('!'),
        to_eof_ok: true,
    }],
    quotes: STYLE_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: STYLE_DIRECTIVES,
    regions: &[],
};

/// SugarSS: indentation-significant CSS with the SCSS comment pair. `css.rs`
/// does not give it Sass's indented-block comment rule, so its `//` does not
/// own the block beneath it.
pub static SUGARSS: Syntax = Syntax {
    name: "sugarss",
    extensions: &["sss"],
    filenames: &[],
    shebangs: &[],
    family: Family::Css,
    engine: Engine::Style,
    line: SLASH_LINE,
    block: STYLE_BLOCK,
    quotes: STYLE_QUOTES,
    heredocs: &[],
    doc: DocStyle::None,
    directives: STYLE_DIRECTIVES,
    regions: &[],
};

/// Style-family descriptors, in registry order.
pub static STYLE: &[(&str, &Syntax)] = &[
    ("css", &CSS),
    ("scss", &SCSS),
    ("sass", &SASS),
    ("less", &LESS),
    ("stylus", &STYLUS),
    ("sugarss", &SUGARSS),
];
