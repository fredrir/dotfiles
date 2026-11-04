//! `core/src/syntax/dash.rs` — dash- and lisp-family descriptors.
//!
//! Paste target: `core/src/syntax/dash.rs`. Requires `core/src/syntax/mod.rs`
//! per SPEC §1. Everything here is `Engine::Generic` unless a doc comment says
//! otherwise.
//!
//! # Where `Engine::Generic` is provably insufficient
//!
//! Three dialects name [`Engine::Dash`] (`engine/dash.rs`) because their run
//! shapes are not table data:
//!
//! 1. **LUA** — long brackets are `[` `=`* `[`, the level is *dynamic* and the
//!    matching close is `]` `=`{level} `]`. The descriptor keeps the level-0
//!    spelling (`--[[`/`]]` and `[[`/`]]`); the engine reads the `=` run from
//!    the opener and builds the matching closer. `---@param …` is deliberately
//!    an ordinary `--` comment: `luadoc` expects the body to begin `-@param`.
//!
//! 2. **HASKELL** — `--` starts a comment only when the *following* byte is not
//!    a Haskell symbol (`!#$%&*+./<=>?@\\^|-~:`). So `-->` and `|--` are
//!    operators, not comments. `At` only inspects the byte *before* the marker,
//!    so no value of `At` can express the rule; the engine checks the dash run.
//!
//! 3. **SQL** — `$tag$ … $tag$` dollar quoting has a dynamic delimiter, which a
//!    `Quote` cannot spell. The engine recognizes it whenever the `$$` entry is
//!    present and honours the block entry's `nested` flag (SQL does not nest).
//!
//! A smaller leak is called out inline: OCaml `{|id|}`-style delimited strings
//! cannot be written as a `Quote` because the delimiter is not static.
//!
//! # Encoding conventions used here
//!
//! * `SPEC §3.1`: at each byte the **first** `LineComment` that matches wins and
//!   the body starts at `at + marker.len()`. Therefore every `line` slice is
//!   ordered **longest marker first** (`///` before `//`) so the doc marker is
//!   not left in the body.
//! * `keep.rs::kept` tests `syntax.directives` with `starts_with` against the
//!   comment body, *not* the trimmed body, and a `BlockComment` body starts
//!   right after `open`. For Haskell that means the body of `{-# LANGUAGE X #-}`
//!   is `"# LANGUAGE X "` — directives are written with the `#` included.
//! * `'` is only encoded as a `Quote` where it cannot also be code: **never** in
//!   Ada (`Integer'Image`), VHDL, MATLAB (`x'` transpose), OCaml/SML (`'a` type
//!   variables), Fortran, or Haskell (`foo'`). See each descriptor.

use crate::syntax::*;

// ---------------------------------------------------------------------------
// LUA
// ---------------------------------------------------------------------------

pub static LUA: Syntax = Syntax {
    name: "lua",
    extensions: &["lua"],
    filenames: &[],
    shebangs: &["lua", "luajit"],
    family: Family::Dash,
    engine: Engine::Dash,
    // LuaLS annotations (`---@param x number`) are ordinary `--` comments: the
    // body starts at the third `-`, which is what `luadoc` parses and what
    // `keep.rs` matches with the global `-@` opening.
    line: &[LineComment {
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // The level-0 spelling. `engine/dash.rs` generalizes the opener to
    // `--[=*[` and the closer to `]=*]` using the level it read. Lua long
    // comments do not nest.
    block: &[BlockComment {
        open: "--[[",
        close: "]]",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // The level-0 spelling; `engine/dash.rs` reads the `=` level from the
        // opener for long strings too.
        Quote {
            open: "[[",
            close: "]]",
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
    doc: DocStyle::Ldoc,
    // LuaLS annotations. VERIFIED in-repo: `--[[@as T]]` appears 12x across
    // `shared/wezterm/utils/append_conf.lua` and `shared/wezterm/plugins/vtabs.lua`
    // — purging those comments breaks the language server's inferred types, so
    // `@as` here is load-bearing, not cosmetic. `---@class/@field/@param/@return`
    // verified in `macos/hammerspoon/utils/spell.lua`.
    directives: &[
        "@as",
        "@cast",
        "@class",
        "@enum",
        "@field",
        "@generic",
        "@meta",
        "@overload",
        "@param",
        "@return",
        "@type",
        "@alias",
        "@deprecated",
        "@diagnostic",
        "@module",
        "@nodiscard",
        "@operator",
        "@package",
        "@private",
        "@protected",
        "@public",
        "@see",
        "@source",
        "@vararg",
        "@version",
    ],
    regions: &[],
};

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

pub static SQL: Syntax = Syntax {
    name: "sql",
    extensions: &["sql", "ddl", "dml"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Dash,
    line: &[LineComment {
        // `engine/dash.rs` treats every `--` as a line comment. MySQL/MariaDB
        // additionally require whitespace after it (`SELECT 1--2` is
        // arithmetic); that dialect needs its own descriptor and a tail rule.
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // `nested: false` is the SQL standard / PostgreSQL shape, and the engine
    // honours it. T-SQL nests — a `Dialect::Tsql` descriptor must be identical
    // except for `nested: true`.
    //
    // `important: Some('!')` keeps MySQL executable comments `/*!40101 ... */`.
    // `/*+` optimizer hints are the same problem and cannot be expressed
    // (one `important` char only) — a fourth `important` slot or a
    // `important: &'static [char]` is the fix.
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        nested: false,
        at: At::Anywhere,
        important: Some('!'),
        to_eof_ok: false,
    }],
    quotes: &[
        // `$tag$ ... $tag$` has a dynamic delimiter. This entry is the empty-tag
        // spelling and its presence tells `engine/dash.rs` to scan any tag;
        // `Quote::raw_tag` is C++-shaped (`R"tag(`/`)tag"`), not SQL.
        Quote {
            open: "$$",
            close: "$$",
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
            open: "'",
            close: "''",
            escape: Escape::Doubling,
            multiline: true,
            // N'…' (national), E'…' (PostgreSQL escape), X'…' (hex),
            // B'…' (bit). Safe: a prefix only matches when the quote is the
            // very next byte.
            prefixes: &["N", "n", "E", "e", "X", "x", "B", "b"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        },
        // Quoted identifier, not a string: recorded so a `--` inside
        // `"weird--name"` is not a comment, but never glyph-swept.
        Quote {
            open: "\"",
            close: "\"\"",
            escape: Escape::Doubling,
            multiline: true,
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

// ---------------------------------------------------------------------------
// HASKELL
// ---------------------------------------------------------------------------

pub static HASKELL: Syntax = Syntax {
    name: "haskell",
    extensions: &["hs"],
    filenames: &[],
    shebangs: &["runhaskell", "runghc"],
    family: Family::Dash,
    engine: Engine::Dash,
    line: &[LineComment {
        // `engine/dash.rs` applies the one-byte lookahead: the dash run must
        // not be followed by a Haskell symbol. `At` cannot express that.
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // `{-# LANGUAGE GADTs #-}` / `{-# INLINE f #-}` are reported as ordinary
    // block comments; `keep.rs` retains them through `directives` below, whose
    // needles start with the `#` that follows `{-`.
    block: &[BlockComment {
        open: "{-",
        close: "-}",
        nested: true,
        at: At::Anywhere,
        important: Some('#'),
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
        // DELIBERATE OMISSION: `'` is not a quote. Haskell does not end
        // identifiers with `'`? It does — `foo'`, `x''` are idiomatic — and a
        // `Quote` has no "not preceded by an identifier byte" guard, so
        // encoding `'` would open a phantom string at every primed name.
        // Cost of omitting: char literals are not glyph-swept. That is safe for
        // comments, because no char literal can contain `--` or `{-`.
    ],
    heredocs: &[],
    // Haddock `-- |` / `-- ^` are doc *comments* with no body to rewrite.
    doc: DocStyle::CommentOnly,
    // Bodies start after `{-`, i.e. at `#`, so the `#` is part of the needle.
    // BNFC/`{-# OPTIONS_GHC -Wall #-}` is covered here too.
    directives: &[
        "# language",
        "# options_ghc",
        "# options_haddock",
        "# inline",
        "# inlinable",
        "# noinline",
        "# specialize",
        "# specialise",
        "# rules",
        "# warning",
        "# deprecated",
        "# complete",
        "# minimal",
        "# overlap",
        "# overlappable",
        "# overlapping",
        "# incoherent",
        "# unpack",
        "# nounpack",
        "# source",
        "# ann",
        "# scc",
        "# static",
        "# phase",
    ],
    regions: &[],
};

// ---------------------------------------------------------------------------
// ADA
// ---------------------------------------------------------------------------

pub static ADA: Syntax = Syntax {
    // `.adb`/`.ads` are the primary form; `.ada`/`.ad` appear in the wild.
    // `.gpr` (GNAT project files) shares the syntax exactly — same descriptor.
    name: "ada",
    extensions: &["adb", "ads", "ada", "ad", "gpr"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[],
    quotes: &[
        // `""` is the Ada escape: "a""b" is a three-character string.
        Quote {
            open: "\"",
            close: "\"\"",
            escape: Escape::Doubling,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
            }),
        },
        // DELIBERATE OMISSION: `'` is the attribute tick — `Integer'Image (X)`,
        // `Vector'Length`, `Float'Digits`. Encoding it as a quote would swallow
        // real code. Character literals must contain exactly one char and can
        // never contain `--`, so skipping them is safe.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // `pragma` is code, not a comment; Ada has no load-bearing comment markers.
    directives: &[],
    regions: &[],
};

// ---------------------------------------------------------------------------
// VHDL
// ---------------------------------------------------------------------------

pub static VHDL: Syntax = Syntax {
    name: "vhdl",
    extensions: &["vhd", "vhdl"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // VHDL-2008 added `/* ... */`. Harmless on VHDL-93 (it is then a syntax
    // error in the source, not a misparse), and it is required by 2008.
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
            close: "\"\"",
            escape: Escape::Doubling,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
            }),
        },
        // DELIBERATE OMISSION: `'` is both a character literal (`'0'`, `'Z'`)
        // and the attribute tick (`clk'event`, `std_logic'image`) and the
        // qualified-expression tick (`integer'(1)`). Ambiguous without a parser.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // Synthesis/tool pragmas are load-bearing: `-- synthesis translate_off`
    // guards non-synthesizable testbench code. Bodies carry a leading space, and
    // `keep.rs` matches untrimmed, so both spellings are listed.
    directives: &[
        " synthesis",
        "synthesis",
        " synopsys",
        "synopsys",
        " pragma",
        "pragma",
        " psl",
        "psl",
        " translate_off",
        "translate_off",
        " translate_on",
        "translate_on",
        " coverage",
        "coverage",
        " altera",
        "altera",
        " xilinx",
        "xilinx",
        " cadence",
        "cadence",
        " mentor",
        "mentor",
    ],
    regions: &[],
};

// ---------------------------------------------------------------------------
// FORTRAN (free form — `.f90` and later)
// ---------------------------------------------------------------------------

pub static FORTRAN: Syntax = Syntax {
    name: "fortran",
    extensions: &["f90", "f95", "f03", "f08", "f18"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "!",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[],
    quotes: &[
        Quote {
            open: "\"",
            close: "\"\"",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &[],
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
            close: "''",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // OpenMP sentinels are directives, in both free and fixed form. Body of
    // `!$omp parallel` is `$omp parallel`.
    directives: &["$omp", "$acc", "$openacc", "dir$", "dec$", "gcc$"],
    regions: &[],
};

/// Fixed-form FORTRAN (`.f`, `.for`, `.f77`, `.ftn`).
///
/// **Not yet registered**: it needs a `Dialect::FortranFixed` variant and a
/// `sniff` rule (SPEC §2) because `.f`/`.for` collide with nothing, but `.inc`
/// does (PHP/Pascal). Listed here so the variant has a descriptor on day one.
///
/// The `C`/`c`/`*` column-1 comments **cannot** be expressed faithfully:
/// `At::LineStart` means "only space/tab before the marker", not "column 1", so
/// `   C = 1` in a free-form file would be eaten. `At::Column1` is the missing
/// variant (~5 lines in `generic.rs`: `at == 0 || source[at - 1] == b'\n'`).
pub static FORTRAN_FIXED: Syntax = Syntax {
    name: "fortran-fixed",
    extensions: &["f", "for", "f77", "ftn"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[
        // UNVERIFIED / lossy: `At::LineStart` is a stand-in for "column 1".
        LineComment {
            marker: "C",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
        LineComment {
            marker: "c",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
        LineComment {
            marker: "*",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
        // Column-1 `!` and trailing `!` are legal in fixed form too (F90+).
        // `!` wins over `C` because `!` is never a valid statement start.
        LineComment {
            marker: "!",
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
            close: "\"\"",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &[],
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
            close: "''",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["$omp", "$acc", "$openacc", "dir$", "dec$", "gcc$"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// MATLAB
// ---------------------------------------------------------------------------

pub static MATLAB: Syntax = Syntax {
    // `.m` collides with Objective-C — `sniff` must disambiguate (SPEC §2).
    name: "matlab",
    extensions: &["m"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "%",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // `%{` / `%}` block comments must sit alone on their line, which is what
    // `At::LineStart` gives us. `At::LineStart` does *not* require the marker to
    // be the whole line, so `%{ not alone` is still treated as an opener —
    // acceptable, since MATLAB rejects that anyway.
    //
    // UNVERIFIED: whether MATLAB nests `%{ %}` blocks. MathWorks documentation
    // does not say and I have no MATLAB source in this repo to test against.
    // `nested: false` matches every lexer I know of (Pygments, Vim, tree-sitter)
    // but this is a guess, not a verification.
    block: &[BlockComment {
        open: "%{",
        close: "%}",
        nested: false,
        at: At::LineStart,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // Double-quoted strings (R2016b+) use `""` as the escape.
        Quote {
            open: "\"",
            close: "\"\"",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
            }),
        },
        // **TRAP — `'` is deliberately NOT a quote.** In MATLAB `'` is also the
        // conjugate-transpose operator: `y = A' * x` is everywhere. A `Quote`
        // with `open: "'"` has no way to require a non-identifier byte before
        // the delimiter, so `A'` would open a phantom string and either swallow
        // the rest of the file or `Bail("unterminated string")` — which turns a
        // syntax error into a refused file. Under-purging (`%` inside a
        // single-quoted literal survives) is the far cheaper failure.
        // Fixing this properly needs `Quote.at` (an `At` for the delimiter),
        // which `mod.rs` does not have.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // `%#ok<AGROW>` (mlint suppression) and `%#codegen` are load-bearing.
    // Bodies start right after `%`.
    directives: &["#ok", "#codegen", "#function", "suppress"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// ELM
// ---------------------------------------------------------------------------

pub static ELM: Syntax = Syntax {
    name: "elm",
    extensions: &["elm"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        // Elm has no operator containing `--`; `--` is always a comment.
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "{-",
        close: "-}",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // Longest first: `"""` is a multi-line string and must not be read as
        // `"` + `""`.
        Quote {
            open: "\"\"\"",
            close: "\"\"\"",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
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
        // Safe here: Elm lower-case identifiers cannot contain `'` (unlike
        // Haskell), so `'` is only ever a char literal.
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
    // `{-| ... -}` is the Elm doc comment; the body of `{-# ... #-}` starts at
    // `#`. `elm-format`'s `{{ unformatted }}`? not a comment.
    directives: &["# elm-format", " elm-format"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// PURESCRIPT
// ---------------------------------------------------------------------------

pub static PURE_SCRIPT: Syntax = Syntax {
    name: "purescript",
    extensions: &["purs"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        // Same operator caveat as Haskell: `--` is fine because PureScript
        // operators cannot contain `--`, but pure `-` operators do exist
        // (`<>`-adjacent). `Anywhere` is the pragmatic choice; revisit if a
        // dedicated engine lands for Haskell.
        marker: "--",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "{-",
        close: "-}",
        nested: true,
        at: At::Anywhere,
        important: Some('#'),
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "\"\"\"",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
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
        // OMITTED: `'` — same reason as Haskell (`foo'` is a legal identifier).
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &[
        "# language",
        "# options_purescript",
        "# inline",
        "# noinline",
    ],
    regions: &[],
};

// ---------------------------------------------------------------------------
// OCAML
// ---------------------------------------------------------------------------

pub static OCAML: Syntax = Syntax {
    name: "ocaml",
    extensions: &["ml", "mli"],
    filenames: &[".ocamlinit"],
    shebangs: &["ocaml"],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[],
    block: &[BlockComment {
        open: "(*",
        close: "*)",
        nested: true,
        at: At::Anywhere,
        // `(*` followed by `*` is an OCamldoc comment, `(**` — same shape as
        // `/**`. Left as `None` on purpose: doc comments must still be purged
        // (SPEC §5 keeps `docstrings` a separate pipeline), so routing them
        // through `important` would defeat `--keep-docstrings = false`.
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // `{| ... |}` quoted string (OCaml 4.02). GAP: `{id| ... |id}` is the
        // same dynamic-delimiter problem as Lua long brackets and is not
        // expressible.
        Quote {
            open: "{|",
            close: "|}",
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
        // OMITTED: `'` — `'a`, `'static` type variables and char literals are
        // indistinguishable without a parser, and type variables are far more
        // common.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// ---------------------------------------------------------------------------
// F#
// ---------------------------------------------------------------------------

pub static FSHARP: Syntax = Syntax {
    // `.fs` collides with GLSL — `sniff` must disambiguate (SPEC §2).
    name: "fsharp",
    extensions: &["fs", "fsi", "fsx", "fsscript"],
    filenames: &[],
    shebangs: &["dotnet"],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[
        // `/// XML doc` — must precede `//` (first match wins).
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
        open: "(*",
        close: "*)",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "\"\"\"",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'"',
            }),
        },
        // `@"verbatim"` (no escapes, `""` doubling), `$"interpolated"`.
        // A prefix only matches when the quote is the next byte.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["@", "$", "@$", "$@"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'"',
            }),
        },
        // OMITTED: `'` — `x'` is a legal F# identifier.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // `#nowarn "40"` and `#light "off"` are compiler directives; in F# they are
    // *code* lines, not comments, but they are also frequently written as
    // trailing comments (`// #nowarn`) during debugging and must survive.
    // Also keeps `#if`/`#else`/`#endif` godwalled blocks.
    directives: &[
        "#nowarn", "#light", "#r", "#load", "#i", "#if", "#else", "#endif", "#time", "#help",
        "#quit", "#indent",
    ],
    regions: &[],
};

// ---------------------------------------------------------------------------
// SML
// ---------------------------------------------------------------------------

pub static SML: Syntax = Syntax {
    name: "sml",
    extensions: &["sml", "sig", "fun"],
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[],
    block: &[BlockComment {
        open: "(*",
        close: "*)",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // `#"a"` is a character literal; `#` as a prefix keeps it out of the
        // way without claiming SML has a `#` operator.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["#"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'"',
            }),
        },
        // OMITTED: `'` — `'a` type variables.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// ---------------------------------------------------------------------------
// Lisp family
//
// `Family::Lisp` for all seven. Assembly and VimL are not Lisps, but they share
// the Lisp *marker shape* (`;` / `"` and no block comments in the common case)
// and SPEC §5 routes `Family::Lisp` through the same `merging()` rule, so they
// belong in this slice rather than in `dash`.
// ---------------------------------------------------------------------------

pub static CLOJURE: Syntax = Syntax {
    name: "clojure",
    extensions: &["clj", "cljs", "cljc", "edn"],
    filenames: &[],
    shebangs: &["clojure", "bb", "babashka"],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[LineComment {
        // `;`, `;;`, `;;;` are all comments; `#_` is a *reader discard* of the
        // next form and must NOT be treated as a comment (it is real syntax, so
        // leaving it out is correct — nothing to encode).
        marker: ";",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "#|",
        close: "|#",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // `#"regex"` — a `#` prefix; no escapes in Clojure regex literals, but
        // treated as escaping so `\"` inside does not end it early.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["#"],
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
    doc: DocStyle::AfterName,
    directives: &[],
    regions: &[],
};

pub static SCHEME: Syntax = Syntax {
    name: "scheme",
    extensions: &["scm", "ss", "sls", "sps", "sld"],
    filenames: &[],
    shebangs: &["guile", "gosh", "chicken", "csi", "scheme", "racket"],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: ";",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // `#| |#` is R6RS/R7RS and nests. `#;` is datum-comment (real syntax) — not
    // encoded, correctly.
    block: &[BlockComment {
        open: "#|",
        close: "|#",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
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
    doc: DocStyle::AfterName,
    directives: &[],
    regions: &[],
};

pub static RACKET: Syntax = Syntax {
    name: "racket",
    extensions: &["rkt", "rktd", "rktl", "scrbl"],
    filenames: &[],
    shebangs: &["racket"],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: ";",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "#|",
        close: "|#",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // `#rx"..."`, `#px"..."` (pregexp), `#\"..."`.
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["#rx", "#px", "#"],
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
    doc: DocStyle::AfterName,
    directives: &[],
    regions: &[],
};

pub static COMMON_LISP: Syntax = Syntax {
    name: "common-lisp",
    extensions: &["lisp", "lsp", "cl", "asd", "ros"],
    filenames: &[],
    shebangs: &["sbcl", "ccl", "ecl", "clisp"],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[LineComment {
        // `;;;;`, `;;;` (top level), `;;` (form), `;` (inline) are all comments.
        // `#;` is reader-discard and is real syntax — correctly not encoded.
        marker: ";",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // CLHS 2.4.4.2: `#| ... |#` blocks nest. `nested: true` for CL as well as
    // for the Schemes, not just Clojure/Racket.
    block: &[BlockComment {
        open: "#|",
        close: "|#",
        nested: true,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
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
    doc: DocStyle::AfterName,
    // `(declaim (optimize (speed 3)))` is code, not a comment.
    directives: &[],
    regions: &[],
};

pub static EMACS_LISP: Syntax = Syntax {
    name: "emacs-lisp",
    extensions: &["el"],
    filenames: &[".emacs", ".emacs.desktop", "abbrev_defs", "history"],
    shebangs: &["emacs"],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: ";",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    // Emacs Lisp has NO `#| |#`. `#@NN` skips NN characters (real syntax, not a
    // comment) — correctly absent.
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
    // UNVERIFIED: `DocStyle::None`, not `AfterName`. An Emacs Lisp docstring is
    // the string after the *argument list*, not after the name:
    // `(defun f (x) "doc" ...)`. `AfterName` will therefore never fire, and
    // docstrings are handled by the ordinary string path. `AfterName` semantics
    // need a "skip one sexp after the name" step to cover Emacs Lisp.
    doc: DocStyle::None,
    // `;;;###autoload`, `;;;###if`, `;;;###chk` — load-bearing for package
    // builds. Also `; -*- lexical-binding: t -*-` file variables are kept by the
    // `-*-` global rule, not here.
    directives: &[";;;###", ";###", "###autoload", "###if", "###chk"],
    regions: &[],
};

pub static ASSEMBLY: Syntax = Syntax {
    // `.s`/`.asm`/`.nasm` cover GAS, NASM and MASM; `.inc` collides with
    // PHP/Pascal and needs `sniff` (SPEC §2).
    name: "assembly",
    extensions: &["s", "S", "asm", "nasm"],
    filenames: &[],
    shebangs: &[],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[
        LineComment {
            // NASM, MASM, TASM, 8086 dialects, and GAS on most targets.
            marker: ";",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
        // GAS also accepts `//` (x86, RISC-V) and `#` (x86 only).
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
        // **TRAP — `#` is deliberately NOT encoded.** On x86 GAS `#` is a
        // comment; on ARM/AArch64 `#` is the immediate-operand prefix
        // (`mov r0, #42`, `add x0, x0, #1`). Encoding it at `Anywhere` would
        // silently delete the rest of every ARM instruction. Encoding it at
        // `LineStart` is no better: `#include`-style macros and `.equ` lines.
        // The correct fix is per-architecture dialects (`Dialect::GasX86` with
        // `#`, `Dialect::GasArm` without), chosen by `sniff` — not a marker
        // table. Likewise `@` is a comment on ARM GAS and an operator elsewhere.
    ],
    // GAS x86 supports `/* */` for multi-line comment blocks; NASM/MASM do not,
    // where `/*` is simply a syntax error, not a misparse.
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
        // NASM single-quoted and backtick strings. Backticks in NASM are
        // escape-processed, `'` is not.
        Quote {
            open: "'",
            close: "",
            escape: Escape::None,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // Assembler directives (`.text`, `.globl`) are code, not comments.
    directives: &[],
    regions: &[],
};

pub static VIML: Syntax = Syntax {
    name: "viml",
    extensions: &["vim", "vimrc"],
    filenames: &[".vimrc", "_vimrc", "vimrc", ".gvimrc", "gvimrc", ".exrc"],
    shebangs: &["vim", "nvim"],
    family: Family::Lisp,
    engine: Engine::Generic,
    line: &[
        // **TRAP — " IS ALSO THE STRING DELIMITER.** This is the single most
        // dangerous descriptor in this file. `"` opens a comment only at the
        // start of a line; everywhere else it opens a double-quoted string
        // (`let s = "hello"`). `At::LineStart` is what makes it correct, and it
        // is only correct because `Quote { open: "\"" }` below claims every
        // non-line-start `"`. Encode either one without the other and VimL
        // breaks in both directions.
        //
        // Residual hazard (UNVERIFIED): a string continued across lines with a
        // leading `\` can put a `"` at line start:
        //     let s = "abc
        //         \ "def"
        // The `At::LineStart` rule fires and eats the rest of the line. Rare
        // enough to accept; a real fix needs the engine to track Vim's `\`
        // continuation.
        LineComment {
            marker: "\"",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
        // Vim9 script (`vim9script`) uses `#` for comments and forbids `"`.
        // `At::LineStart` is used for the same reason as above; a
        // `Dialect::Vim9` should carry `#` only.
        LineComment {
            marker: "#",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,

            owns_indent_block: false,
        },
    ],
    block: &[],
    quotes: &[
        // Vim single-quoted strings: `''` is the ONLY escape, no backslashes.
        Quote {
            open: "'",
            close: "''",
            escape: Escape::Doubling,
            multiline: false,
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
    ],
    heredocs: &[],
    doc: DocStyle::None,
    // Modelines: `" vim: set ts=2 sw=2:` at the top or bottom of the file.
    // `detect` also reads these (SPEC §2) — they must survive purging or the
    // next tool run loses the file's settings. Bodies start right after `"`.
    directives: &["vim:", "vi:", "ex:", "vim<", "vim>", "vim9script"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// Slices for `LANGUAGES` / `Dialect::syntax()`
// ---------------------------------------------------------------------------

/// Dash-family descriptors, in registry order.
pub static DASH: &[(&str, &Syntax)] = &[
    ("lua", &LUA),
    ("sql", &SQL),
    ("haskell", &HASKELL),
    ("ada", &ADA),
    ("vhdl", &VHDL),
    ("fortran", &FORTRAN),
    ("matlab", &MATLAB),
    ("elm", &ELM),
    ("purescript", &PURE_SCRIPT),
    ("ocaml", &OCAML),
    ("fsharp", &FSHARP),
    ("sml", &SML),
];

/// Lisp-family descriptors, in registry order.
pub static LISP: &[(&str, &Syntax)] = &[
    ("clojure", &CLOJURE),
    ("scheme", &SCHEME),
    ("racket", &RACKET),
    ("common-lisp", &COMMON_LISP),
    ("emacs-lisp", &EMACS_LISP),
    ("assembly", &ASSEMBLY),
    ("viml", &VIML),
];
