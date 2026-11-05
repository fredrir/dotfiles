
use crate::syntax::*;

pub static JSONC: Syntax = Syntax {
    name: "jsonc",
    extensions: &["jsonc"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
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
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

pub static JSON5: Syntax = Syntax {
    name: "json5",
    extensions: &["json5"],
    filenames: &[".babelrc", ".eslintrc", ".swcrc", ".jshintrc4"],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
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
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

pub static CONFIG: &[(&str, &Syntax)] = &[("jsonc", &JSONC), ("json5", &JSON5)];
