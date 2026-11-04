//! Syntax descriptors for every brace-delimited (`Family::Curly`) dialect.
//!
//! This table is the **single source of truth** for the curly family: `lang.rs`
//! derives `Dialect::syntax()`, `LANGUAGES`, `Dialect::ALL` and the registry test
//! from these statics, so a new dialect is one entry here plus one enum variant
//! and nothing else can drift. The generic engine is table-driven: it reads
//! `line` / `block` / `quotes` and never branches on the dialect name.
//!
//! Contracts this table is written against — the engine must honour all four:
//!   1. line comments: list order, longest/doc-bearing markers first (`///` before
//!      `//`), because `LineComment.doc` is only reported for the entry that
//!      matched.
//!   2. block comments: longest `open` wins (`/+` before `/*`).
//!   3. quotes, prefixes: `prefixes` are only listed where the prefix changes the
//!      *scanning* semantics (raw, verbatim, raw_tag, `$/`). `L"…"`, `u8"…"`,
//!      `b"…"`, `b'c'` are byte-identical to their bare form, so they get no
//!      entry: the prefix byte is skipped and the bare quote matches. Every
//!      descriptor lists its bare (empty-`prefixes`) quotes first, so the table
//!      behaves the same whether an engine lets a prefixed entry fall back to a
//!      bare match or requires the prefix.
//!   4. quotes, hashes: for `hashes: true` the count `N` is the maximal run of
//!      `#` immediately before `open` — Rust `r##"…"##` (the run follows the
//!      `r`) and Swift `#"…"#` (the run *is* the `#`, so Swift needs no prefix
//!      entry). The literal then closes on `close` followed by `N` hashes.
//!
//! Known limits, accepted and marked `UNVERIFIED` where they are per-dialect:
//! interpolated literals (C# `$"…"`, Kotlin `"${…}"`, Swift `"\(…)"`, Dart
//! `"${…}"`) can hide a `"` inside a hole; Groovy `/…/` slashy strings, D
//! `q"{…}"`/`q"(…)"` token strings and Zig `\\` line strings need an engine hook.
//!
//! `important` is the "this survives the purge" form of a block comment
//! (`/*!` Doxygen, `/**` natspec). It mirrors today's `keep.rs`, which keeps
//! `/*!` for every dialect and nothing else: the C/Objective-C/Doxygen languages
//! keep `!`, Solidity keeps `*` (natspec `/** */` is load-bearing there and
//! `/*!` does not exist), and the `/**`-is-just-javadoc languages
//! (Java/Kotlin/Swift/C#/Scala/Dart/Groovy/Haxe) keep nothing, so javadoc/KDoc
//! is still purged. See SPEC §1/§6 if that decision is ever revisited.

use crate::syntax::*;

/// Rust: nested block comments, `r#"…"#` raw strings, byte strings/chars.
/// Note for the engine: `'` is both a char literal and a lifetime/loop label.
/// The descriptor cannot express that heuristic — the engine needs the existing
/// `rust_quote()` lookahead (`'` + char + `'`, or `'\…`) or `&'a str` parses as
/// an unterminated char literal. `/\*\*/` nesting is real (Rust reference).
pub static RUST: Syntax = Syntax {
    name: "rust",
    extensions: &["rs"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//!",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: true,
        at: At::Anywhere,
        important: Some('!'),
        to_eof_ok: false,
    }],
    quotes: &[
        // "…" — backslash escapes, may span lines. b"…" behaves identically, so
        // it needs no prefix entry ("b" is skipped as an identifier byte).
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
        // 'c' / b'c' — see the lifetime note above.
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        // r"…", r#"…"#, br##"…"## — the `#` run is counted immediately before
        // `open`, and the close is `"` followed by the same count.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &["r", "br"],
            hashes: true,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["clippy::", "rustfmt::skip", "#__pure__", "@__pure__"],
    regions: &[],
};

/// Go: `//`, non-nesting `/* */`, backtick raw strings, rune literals.
pub static GO: Syntax = Syntax {
    name: "go",
    extensions: &["go"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
            open: "`",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'`',
            }),
        },
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
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["go:", "+build", "nolint"],
    regions: &[],
};

/// C: trailing-`\` line comments continue, non-nesting blocks, `L"…"`/`u8"…"`.
pub static C: Syntax = Syntax {
    name: "c",
    extensions: &["c", "h"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: true,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: Some('!'),
        to_eof_ok: false,
    }],
    quotes: &[
        // "…", L"…", u8"…", u"…", U"…" are all the same literal to a lexer, so
        // only the bare form is described; the prefix is skipped as an
        // identifier byte and the `"` matches here.
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
        // 'c', L'c', u'c', U'c'.
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["pragma:", "clang-format", "nolint", "iwyu pragma:"],
    regions: &[],
};

/// C++: C plus `R"tag(… )tag"` and the raw prefixes, plus `//` continuing.
pub static CPP: Syntax = Syntax {
    name: "cpp",
    extensions: &[
        "cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "ipp", "inl", "ixx", "tcc", "tpp",
    ],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: true,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: Some('!'),
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
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        // R"(…)", R"tag(… )tag", u8R"(…)" — closes on `)` + tag + `"`.
        // The plain `"` entry above is listed first so a bare quote is never
        // captured by the raw form.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &["u8R", "uR", "UR", "LR", "R"],
            hashes: false,
            raw_tag: true,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["pragma:", "clang-format", "nolint", "iwyu pragma:"],
    regions: &[],
};

/// Java: javadoc `/** */` is purged like any comment, text blocks are `"""`.
/// UNVERIFIED: C++-style numeric digit separators (`1'000'000`) do not apply to
/// Java, but its `'` char literal does, so `'` is safe here.
pub static JAVA: Syntax = Syntax {
    name: "java",
    extensions: &["java"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
            open: "\"\"\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
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
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &[
        "noinspection",
        "checkstyle",
        "formatter:off",
        "spotbugs:",
        "pmd",
        "suppresswarnings",
    ],
    regions: &[],
};

/// C#: `@"…"` verbatim (doubling), `$"…"` interpolated, `"""…"""` raw (C# 11).
/// UNVERIFIED: raw strings with more than three quotes (`""""…""""`) and nested
/// interpolation holes (`$"{f("x")}"`) are not expressible; a hole containing a
/// `"` would end the string early. Rare in practice.
pub static CSHARP: Syntax = Syntax {
    name: "csharp",
    extensions: &["cs", "csx"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // """…""" (C# 11 raw) must precede the plain `"` entry.
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
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
            text: None,
        },
        // @"…" / @$"…" / $@"…" verbatim — a doubled `""` is a literal quote and
        // the literal may span lines.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &["@$", "$@", "@"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'"',
            }),
        },
        // $"…" interpolated.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["$"],
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
    doc: DocStyle::CommentOnly,
    directives: &[
        "nullable",
        "pragma warning",
        "#pragma",
        "#region",
        "#endregion",
        "analyzer",
        "resharper",
        "suppressmessage",
    ],
    regions: &[],
};

/// Kotlin: block comments nest, raw `"""…"""` has no escapes, backtick
/// identifiers are a quote with `text: None` (recognised, never glyph-swept).
pub static KOTLIN: Syntax = Syntax {
    name: "kotlin",
    extensions: &["kt", "kts"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
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
            text: None,
        },
        // `fun `with space`()` — a name, not a string.
        Quote {
            open: "`",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["ktlint-disable", "noinspection", "detekt:", "formatter:off"],
    regions: &[],
};

/// Swift: blocks nest, `"""…"""` multi-line, `#"…"#` raw (hash-counted; the `#`
/// run before the quote is the count, so no prefix entry is needed).
/// Swift has no character literal, so `'` is deliberately absent.
pub static SWIFT: Syntax = Syntax {
    name: "swift",
    extensions: &["swift"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
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
        // #"…"# and #"""…"""# — the `#` run before `open` is the hash count
        // (Swift's `#` prefix IS that run, so it needs no `prefixes` entry).
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: true,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: true,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'"',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["swiftlint:", "mark:", "sourcery:", "swift-format", "nodoc"],
    regions: &[],
};

/// Zig: **no block comments at all** — only `//`, `///` and `//!`.
/// UNVERIFIED: Zig multi-line string literals (`\\` at the start of a line, body
/// to end of line) cannot be expressed as a `Quote` (they are line-scoped and
/// have no closing delimiter). Without an engine hook, a `//` inside a `\\`
/// line is treated as a comment. The old `curly.rs::zig_line_string` must
/// survive the port.
pub static ZIG: Syntax = Syntax {
    name: "zig",
    extensions: &["zig"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//!",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[],
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
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

/// Dart: blocks nest, `'''`/`"""` multi-line, `r'…'`/`r"…"` raw single-line.
/// Raw entries are listed first so the `r` prefix wins over the bare quote.
pub static DART: Syntax = Syntax {
    name: "dart",
    extensions: &["dart"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &["r"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "'''",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &["r"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "'''",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &["r"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
            }),
        },
        Quote {
            open: "'",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &["r"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        },
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
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &[
        "dart format off",
        "ignore:",
        "dart analyze",
        "coverage:ignore",
        "analyzer:",
    ],
    regions: &[],
};

/// Scala: blocks nest, `"""…"""` raw multi-line, `'c'` char literals.
pub static SCALA: Syntax = Syntax {
    name: "scala",
    extensions: &["scala", "sc", "sbt"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
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
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["scalafmt:", "scalafix:", "noinspection", "nowarn"],
    regions: &[],
};

/// Groovy: `'''`/`"""` multi-line, `$/…/$` dollar-slashy, `@Grab` on filenames.
/// UNVERIFIED: plain slashy strings `/…/` are NOT representable — a `/` quote
/// would swallow division (`a / b`) and bail as an unterminated string. Groovy
/// therefore still needs an engine hook (or an explicit accepted risk) for
/// slashy literals. `over /* */` nesting: UNVERIFIED — treated as non-nesting,
/// matching the Java grammar it was derived from.
pub static GROOVY: Syntax = Syntax {
    name: "groovy",
    extensions: &["groovy", "gvy", "gy", "gsh"],
    filenames: &[
        "build.gradle",
        "settings.gradle",
        "init.gradle",
        "Jenkinsfile",
    ],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
            open: "\"\"\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "'''",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: 0,
            }),
        },
        Quote {
            open: "$/",
            close: "/$",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
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
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "groovylint-disable",
        "noinspection",
        "spotbugs:",
        "formatter:off",
    ],
    regions: &[],
};

/// Solidity: natspec `///` and `/** */` survive (`nodoc` opts out), blocks do
/// not nest, `unicode"…"`/`hex"…"` are prefixed literals.
pub static SOLIDITY: Syntax = Syntax {
    name: "solidity",
    extensions: &["sol"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: Some('*'),
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
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        // unicode"…" / hex"…" — same closing quote, prefixed form.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["unicode", "hex"],
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
            prefixes: &["unicode", "hex"],
            hashes: false,
            raw_tag: false,
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "solhint-disable",
        "nodoc",
        "slither-disable",
        "forgefmt",
        "prettier-ignore",
    ],
    regions: &[],
};

/// Objective-C: non-nesting blocks, `@"…"` NSString literals with escapes.
pub static OBJECTIVE_C: Syntax = Syntax {
    name: "objective-c",
    extensions: &["m", "mm", "h"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: true,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: Some('!'),
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
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        // @"…" — NSString literal, escapes processed as usual.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["@"],
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
    directives: &["pragma mark", "pragma:", "clang-format", "nolint"],
    regions: &[],
};

/// Vala: C#-derived, `"""…"""` multi-line, `@"…"` verbatim (no escapes).
/// UNVERIFIED: block-comment nesting. `valac`'s lexer is believed to support
/// nested `/* /* */ */`; treated as nesting so a nested block cannot swallow
/// live code. Verify against `valac` source before shipping.
pub static VALA: Syntax = Syntax {
    name: "vala",
    extensions: &["vala", "vapi"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
        nested: true,
        at: At::Anywhere,
        important: Some('!'),
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        },
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
            text: None,
        },
        // @"…" verbatim: no escape processing, may span lines.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &["@"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'"',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["valadoc:", "noinspection"],
    regions: &[],
};

/// D: `/* */` does NOT nest but `/+ +/` DOES; `r"…"`, `x"…"`, backtick raw,
/// `q"…"` token strings. `///` and `/** */` are ddoc.
/// UNVERIFIED: `q"(…)"` / `q"[…]"` / `q"IDENT … IDENT"` token strings — the
/// bracket form balances delimiters and can contain a bare `"`. Encoded as a
/// plain no-escape quote, so a `"` inside such a token string ends it early.
/// This is the one D form the table cannot express; keep it in mind for review.
pub static D: Syntax = Syntax {
    name: "d",
    extensions: &["d", "di"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[
        // `/+ … +/` nests; longest open wins over `/*`.
        BlockComment {
            open: "/+",
            close: "+/",
            nested: true,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "/*",
            close: "*/",
            nested: false,
            at: At::Anywhere,
            important: Some('!'),
            to_eof_ok: false,
        },
    ],
    quotes: &[
        // q"…" — see the UNVERIFIED note above.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &["q"],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        // r"…" and x"…" — raw / hex strings.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &["r", "x"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'"',
            }),
        },
        Quote {
            open: "`",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'`',
            }),
        },
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
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["dub:", "nolint", "safety:"],
    regions: &[],
};

/// Haxe: `"…"` and `'…'` are both strings, blocks do not nest.
/// UNVERIFIED: Haxe block-comment nesting. The manual is silent; non-nesting is
/// assumed (C-grammar default) so a stray `/*` inside a comment cannot swallow
/// live code. Flip to `true` if the Haxe lexer is confirmed to nest.
pub static HAXE: Syntax = Syntax {
    name: "haxe",
    extensions: &["hx"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
        important: Some('!'),
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
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["haxelib:", "noinspection"],
    regions: &[],
};

/// Verilog / SystemVerilog: `//` and non-nesting `/* */`. There is no character
/// literal — `4'b1010` uses `'` as a size/base separator, so `'` is deliberately
/// absent from `quotes` (a `'` quote would eat the rest of the line).
pub static VERILOG: Syntax = Syntax {
    name: "verilog",
    extensions: &["v", "sv", "vh", "svh"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
    quotes: &[Quote {
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
    }],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "pragma",
        "synthesis",
        "synopsys",
        "verilator",
        "lint_off",
        "lint_on",
        "translate_off",
        "translate_on",
    ],
    regions: &[],
};

/// WGSL: block comments **nest**, `///` and `/** */` are doc comments, and the
/// only escape sequences are `\\` and `\"`.
pub static WGSL: Syntax = Syntax {
    name: "wgsl",
    extensions: &["wgsl"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: true,
        at: At::Anywhere,
        important: Some('*'),
        to_eof_ok: false,
    }],
    quotes: &[Quote {
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
    }],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

/// GLSL: `//`, non-nesting `/* */`. GLSL has no string escape sequences, so
/// `Escape::None` — a `\` in a `#include "…"` path is literal.
pub static GLSL: Syntax = Syntax {
    name: "glsl",
    extensions: &[
        "glsl", "vert", "frag", "geom", "comp", "tesc", "tese", "rgen", "rint", "rmiss", "rchit",
        "rahit", "vsh", "fsh", "gsh", "vs", "fs",
    ],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
    quotes: &[Quote {
        open: "\"",
        close: "",
        escape: Escape::None,
        multiline: false,
        prefixes: &[],
        hashes: false,
        raw_tag: false,
        text: None,
    }],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["pragma"],
    regions: &[],
};

/// Gleam: **`//` only, no block comments**, and string literals may contain
/// real newlines. `///` is a doc comment, `////` a module doc.
pub static GLEAM: Syntax = Syntax {
    name: "gleam",
    extensions: &["gleam"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[
        LineComment {
            marker: "////",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "///",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: true,

            owns_indent_block: false,
        },
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[],
    quotes: &[Quote {
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
    }],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

/// Odin: blocks nest, `` `raw` `` strings, `'r'` runes.
/// UNVERIFIED: Odin block-comment nesting — `odin`'s scanner is believed to nest
/// `/* /* */ */`. Encoded as nesting; confirm against `odin/src/tokenizer.cpp`.
pub static ODIN: Syntax = Syntax {
    name: "odin",
    extensions: &["odin"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "`",
            close: "",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'`',
            }),
        },
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
            text: None,
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["odin:"],
    regions: &[],
};

/// V: `'…'` and `"…"` are both strings (multi-line allowed), `r'…'` raw,
/// `c'…'` a char. Blocks nest per the V docs.
/// UNVERIFIED: V block-comment nesting — `vlib/v/scanner` nests `/* */`.
/// Encoded as nesting; confirm before shipping.
pub static V: Syntax = Syntax {
    name: "v",
    extensions: &["v", "vsh"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
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
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // '…' and "…" are ordinary, multi-line-capable strings in V.
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
        // r'…' raw string, c'…' character literal, r"…" raw string.
        Quote {
            open: "'",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &["r"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        },
        Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["c"],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        Quote {
            open: "\"",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &["r"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["vfmt", "v: "],
    regions: &[],
};

/// Roc: **`#` line comments only, no block comments, no braces.** Roc is
/// layout-based like Elm, so `Family::Curly` here is a public-API compromise
/// (the dialect enum has no Indented family yet); the purger only cares that
/// the comment markers are right. Flagged in the report: Roc probably belongs
/// in `Family::Hash`.
pub static ROC: Syntax = Syntax {
    name: "roc",
    extensions: &["roc"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Curly,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[],
    quotes: &[Quote {
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
    }],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

/// The curly family, keyed by `Syntax::name`. `lang.rs`/`registry.rs` derive
/// `Dialect::syntax()`, `LANGUAGES`, `Dialect::ALL` and the registry test from
/// this one list, so descriptors and dialects cannot drift apart.
pub static CURLY: &[(&str, &Syntax)] = &[
    ("rust", &RUST),
    ("go", &GO),
    ("c", &C),
    ("cpp", &CPP),
    ("java", &JAVA),
    ("csharp", &CSHARP),
    ("kotlin", &KOTLIN),
    ("swift", &SWIFT),
    ("zig", &ZIG),
    ("dart", &DART),
    ("scala", &SCALA),
    ("groovy", &GROOVY),
    ("solidity", &SOLIDITY),
    ("objective-c", &OBJECTIVE_C),
    ("vala", &VALA),
    ("d", &D),
    ("haxe", &HAXE),
    ("verilog", &VERILOG),
    ("wgsl", &WGSL),
    ("glsl", &GLSL),
    ("gleam", &GLEAM),
    ("odin", &ODIN),
    ("v", &V),
    ("roc", &ROC),
];
