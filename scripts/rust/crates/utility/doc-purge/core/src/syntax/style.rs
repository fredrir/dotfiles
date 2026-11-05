
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

const SLASH_LINE: &[LineComment] = &[LineComment {
    marker: "//",
    at: At::Anywhere,
    continues_on_backslash: false,
    doc: false,
    owns_indent_block: false,
}];

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

pub static STYLE: &[(&str, &Syntax)] = &[
    ("css", &CSS),
    ("scss", &SCSS),
    ("sass", &SASS),
    ("less", &LESS),
    ("stylus", &STYLUS),
    ("sugarss", &SUGARSS),
];
