//! `#`-comment dialects: scripting, config, build and infra.
//!
//! Every descriptor here is `Family::Hash` (see `SPEC.md` §1: `merging()` is only
//! consulted for `Curly | Hash | Dash | Lisp`).
//!
//! Evidence for the `At` choice of the shell family is in the dotfiles repo:
//! `shared/zsh/90-utils.zsh:33` (`$#`), `shared/zsh/49-wezterm.zsh:79`
//! (`"${#jobstates}"`), `shared/zsh/49-wezterm.zsh:245` (`${reply#*:}`),
//! `shared/zsh/50-fzf.zsh:13` (`${...}:#reset`), `shared/zsh/02-utils.zsh:51`
//! (`#c1` base prefix inside `(( … ))`). The first two prove `At::Anywhere` is
//! wrong; the last two prove `At::WordStart` alone is *also* wrong and that these
//! dialects must run on `Engine::Shell`, which owns the `$`/`${…}`/`$(…)`/`((…))`
//! state that a single `At` predicate cannot express.

use crate::syntax::*;

// -- python -----------------------------------------------------------------

// `SPEC.md` §1 names only `r b u f rb br`; CPython accepts every case pairing and
// the `br`/`rb`/`fr`/`rf` permutations, so all of them are listed. Longest first.
// UNVERIFIED: `Quote` cannot turn `escapes` off for the raw prefixes, so a `\"`
// inside `r"…"` is read as an escape (the old `hash.rs::python` passed `!raw`).
// A `r"…\\"`-shaped source can therefore scan one quote too far.
const PY_PREFIXES: &[&str] = &[
    "rb", "br", "Rb", "rB", "bR", "Br", "RB", "BR", "fr", "rf", "Fr", "fR", "rF", "Rf", "RF", "r",
    "R", "b", "B", "u", "U", "f", "F",
];

pub static PYTHON: Syntax = Syntax {
    name: "python",
    extensions: &["py", "pyi", "pyw", "pyx", "pxd"],
    filenames: &[],
    shebangs: &["python", "python3"],
    family: Family::Hash,
    engine: Engine::Indented,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
    quotes: &[
        Quote {
            open: "\"\"\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: PY_PREFIXES,
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'"',
            }),
        },
        Quote {
            open: "'''",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: PY_PREFIXES,
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
            prefixes: PY_PREFIXES,
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
            prefixes: PY_PREFIXES,
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
    doc: DocStyle::Body {
        openers: &["def ", "async def ", "class ", ""],
        delim: "\"\"\"",
    },
    directives: &[
        "type:", "noqa", "pylint:", "pyright:", "mypy:", "ruff:", "fmt:", "isort:", "nosec",
        "pragma:", "vulture:", "pyre-", "pytype:", "coding:", "cython:",
        // From SPEC.md §6, which is authoritative over this task list.
        "no-cover",
    ],
    regions: &[],
};

// -- toml -------------------------------------------------------------------

pub static TOML: Syntax = Syntax {
    name: "toml",
    extensions: &["toml"],
    filenames: &[
        "Cargo.toml",
        "pyproject.toml",
        "Pipfile",
        "rust-toolchain.toml",
    ],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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
                delim: b'"',
            }),
        },
        Quote {
            open: "'''",
            close: "",
            escape: Escape::None,
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
    directives: &[],
    regions: &[],
};

// -- yaml -------------------------------------------------------------------

// `hash.rs::yaml` opens a comment on `#` at the indent or after a space only;
// `WordStart` widens that to "any byte that is not an ident byte", so a
// `key: value:#not-a-comment` in YAML is purged here but kept by the old scanner.
// UNVERIFIED: no such file exists in the repo to settle which is wanted; the
// over-match is the safe direction (purging a stray `#` word, never losing code).
pub static YAML: Syntax = Syntax {
    name: "yaml",
    extensions: &["yaml", "yml"],
    filenames: &[],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Indented,
    line: &[LineComment {
        marker: "#",
        at: At::WordStart,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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
    directives: &["yamllint "],
    regions: &[],
};

// -- elixir -----------------------------------------------------------------

// Elixir heredocs are `"""`; sigils take their delimiter per-call. `~s`/`~c`/`~w`
// interpolate and are swept, `~r` is regex-ish and must not be glyph-swept.
// Per SPEC.md §1 step 3 a `Quote` matches when its `open` matches directly *or*
// when a prefix plus `open` matches, so the `"`-with-sigil-prefixes entry also
// serves a bare `"` and no separate plain entry is needed.
// UNVERIFIED: sigils with an arbitrary punctuation delimiter (`~s|…|`, `~s#…#`)
// are only covered for `|`; `~s#…#` is a legal Elixir sigil and its `#` opening
// delimiter would be read as a comment.
pub static ELIXIR: Syntax = Syntax {
    name: "elixir",
    extensions: &["ex", "exs"],
    filenames: &[],
    shebangs: &["elixir"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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
                delim: b'"',
            }),
        },
        Quote {
            open: "(",
            close: ")",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["~s", "~S", "~c", "~C", "~w", "~W"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'(',
            }),
        },
        Quote {
            open: "[",
            close: "]",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["~s", "~S", "~c", "~C", "~w", "~W"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'[',
            }),
        },
        Quote {
            open: "{",
            close: "}",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["~s", "~S", "~c", "~C", "~w", "~W"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'{',
            }),
        },
        Quote {
            open: "<",
            close: ">",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["~s", "~S", "~c", "~C", "~w", "~W"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'<',
            }),
        },
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["~s", "~S", "~c", "~C", "~w", "~W"],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'"',
            }),
        },
        Quote {
            open: "/",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["~r", "~R"],
            hashes: false,
            raw_tag: false,
            text: None,
        },
        Quote {
            open: "|",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["~r", "~R"],
            hashes: false,
            raw_tag: false,
            text: None,
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
    doc: DocStyle::Attribute {
        attributes: &["@moduledoc", "@doc", "@typedoc", "@shortdoc"],
    },
    directives: &["credo:disable-for-next-line", "credo:disable-for-this-file"],
    regions: &[],
};

// -- crystal ----------------------------------------------------------------

// UNVERIFIED: Crystal `%w(…)`, `%q(…)`, `%Q(…)`, `%i(…)` and `%(…)` percent
// literals cannot be expressed as `Quote` entries (no prefix list reaches a
// `%`-started delimiter with balanced nesting), so they are not declared and a
// `#` inside one would be read as a comment.
pub static CRYSTAL: Syntax = Syntax {
    name: "crystal",
    extensions: &["cr"],
    filenames: &[],
    shebangs: &["crystal"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: true,
                folds: false,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["ameba:disable", "ameba:enable"],
    regions: &[],
};

// -- nim --------------------------------------------------------------------

// `r"…"` is raw; `Quote` cannot switch `escapes` off per prefix, so the raw form
// shares the `"` entry and a `\"` inside `r"…"` will be read as an escape.
// UNVERIFIED: Nim also has `fmt"…"`, `&"…"` and other string macros.
pub static NIM: Syntax = Syntax {
    name: "nim",
    extensions: &["nim", "nims", "nimble"],
    filenames: &[],
    shebangs: &["nim"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        // `##` is a doc comment; `#[ … ]#` below is the doc/block form.
        doc: true,
    
    owns_indent_block: false,}],
    block: &[BlockComment {
        // SPEC.md §1 names Nim `#[ ]#` as a NESTED block comment.
        open: "#[",
        close: "]#",
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
                delim: b'"',
            }),
        },
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["r", "R"],
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
                folds: false,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &[],
    regions: &[],
};

// -- perl -------------------------------------------------------------------

// Perl is Ruby's sibling, not a `Generic` dialect: POD (`=pod`/`=head1`/`=item`
// … up to `=cut` at column 0), the `q qq qw qr m s tr y` quote-like operators,
// `/…/` regex disambiguation and heredocs. `Engine::Ruby` is declared because a
// dedicated literal engine is required and Generic cannot express any of it.
// UNVERIFIED: engine/ruby.rs currently hard-codes `=begin`/`=end` and `%`-sigils;
// it must be parameterised (POD alpha keywords, `q`-operators) or Perl needs its
// own engine/perl.rs. The `quotes` below only cover the plain forms.
pub static PERL: Syntax = Syntax {
    name: "perl",
    extensions: &["pl", "pm", "t", "psgi"],
    filenames: &[],
    shebangs: &["perl"],
    family: Family::Hash,
    engine: Engine::Perl,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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
    ],
    heredocs: &[Heredoc {
        openers: &["<<", "<<~"],
        strip_tabs: false,
    }],
    // POD is a comment form, not a string-doc form.
    doc: DocStyle::CommentOnly,
    directives: &["no critic", "perltidy"],
    regions: &[],
};

// -- r ----------------------------------------------------------------------

// UNVERIFIED: R raw strings `r"(…)"`, `R'--[… ]--'` allow up to 16 dashes between
// the quote and the bracket; `Quote::raw_tag` is `R"tag( … )tag"`-shaped and does
// not cover it, so raw strings are not declared and a `#` inside one would be
// purged. Plain R strings cannot span lines.
pub static R: Syntax = Syntax {
    name: "r",
    extensions: &["r", "R"],
    filenames: &[".Rprofile"],
    // SPEC.md §1 spells it `Rscript`; the interpreter name is capitalised on
    // every system R installs, but be tolerant of a lowercased shebang too.
    shebangs: &["Rscript", "rscript"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        // `#'` is the roxygen doc comment.
        continues_on_backslash: false,
        doc: true,
    
    owns_indent_block: false,}],
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
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::CommentOnly,
    directives: &["nolint", "styler:"],
    regions: &[],
};

// -- julia ------------------------------------------------------------------

// `#=` … `=#` is nested. Julia's `'` is both a char literal and the adjoint
// operator (`A'`, `x'`), which no `Quote` can disambiguate — omitting it means an
// adjoint is scanned as ordinary code and a char literal is not swept.
// UNVERIFIED: trade-off is deliberate; if `'` is added instead, every adjoint
// (`A'`, `b'`, `(a*b)'`) bails with "unterminated string".
// UNVERIFIED: Julia string macros (`raw"…"`, `b"…"`, `L"…"`, `r"…"`) are bare
// prefixes like Nim's and are not declared.
pub static JULIA: Syntax = Syntax {
    name: "julia",
    extensions: &["jl"],
    filenames: &[],
    shebangs: &["julia"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        // `#:` doc-comment form (Julia ≥1.11) and `##` both read as docs.
        doc: true,
    
    owns_indent_block: false,}],
    block: &[BlockComment {
        open: "#=",
        close: "=#",
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
    ],
    heredocs: &[],
    doc: DocStyle::Before {
        keywords: &["function ", "struct ", "macro ", "const "],
    },
    directives: &["jl: ", "julia: "],
    regions: &[],
};

// -- powershell -------------------------------------------------------------

// PowerShell escapes with a backtick, not a backslash; a doubled quote is the
// only escape inside a non-expandable string, so both quotes use `Doubling`.
// Here-strings (`@"` … `"@`, `@'` … `'@`) are declared as quotes; note they only
// legally close when `"@`/`'@` starts a line, which `Quote` cannot require.
// UNVERIFIED: here-string close is not line-anchored, and the same descriptors
// do not model `${…}`/`$()` subexpressions.
pub static POWERSHELL: Syntax = Syntax {
    name: "powershell",
    extensions: &["ps1", "psm1", "psd1"],
    filenames: &[],
    shebangs: &["pwsh", "powershell"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[BlockComment {
        // `<# … #>` is non-nesting. It is also how comment-based help is written
        // (`.SYNOPSIS`), which `keep.rs` must not treat as a doc rewrite.
        open: "<#",
        close: "#>",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        Quote {
            open: "@\"",
            close: "\"@",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'"',
            }),
        },
        Quote {
            open: "@'",
            close: "'@",
            escape: Escape::None,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: false,
                delim: b'\'',
            }),
        },
        Quote {
            open: "\"",
            close: "",
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
            close: "",
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
    directives: &[
        "requires -",
        "psavoidusing",
        "psuseapprovedverbs",
        "psscriptanalyzer",
    ],
    regions: &[],
};

// -- tcl --------------------------------------------------------------------

// `#` is only a comment where a command can start, so `LineStart` is the closest
// the descriptor can get; Tcl's other legal form is `;#` trailing a command, which
// this misses. Braces are bare words, not strings.
// UNVERIFIED: `;#` trailing comments are not scanned.
pub static TCL: Syntax = Syntax {
    name: "tcl",
    extensions: &["tcl", "tk"],
    filenames: &[],
    shebangs: &["tclsh", "wish"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::LineStart,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
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

// -- fish -------------------------------------------------------------------

// Fish has no `$#` and no `${x#pat}` expansion form, so `WordStart` is both
// correct and sufficient here — `abc#def` is one word, `cmd # note` is a comment.
// UNVERIFIED: fish's own tokenizer also ends a word at `;`/`&`/`|`, which
// `WordStart`'s previous-byte test does not model; `;$#`-like sequences are rare.
pub static FISH: Syntax = Syntax {
    name: "fish",
    extensions: &["fish"],
    filenames: &[],
    shebangs: &["fish"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::WordStart,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
    quotes: &[
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
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// -- awk --------------------------------------------------------------------

// UNVERIFIED: POSIX awk regex literals `/…\/…/` are not declared; a `#` inside a
// regex (`/^a#b/`) is scanned as a comment. `gsub`/`match` arguments written as
// strings are fine.
pub static AWK: Syntax = Syntax {
    name: "awk",
    extensions: &["awk"],
    filenames: &[],
    shebangs: &["awk", "gawk", "mawk", "nawk"],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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

// -- gdscript ---------------------------------------------------------------

// `##` is the doc-comment form (surfaced as `doc: true`); `@export`-style
// annotations are code. GDScript is an indented-body language like Python.
pub static GDSCRIPT: Syntax = Syntax {
    name: "gdscript",
    extensions: &["gd"],
    filenames: &[],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: true,
    
    owns_indent_block: false,}],
    block: &[],
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
                delim: b'"',
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
                delim: b'\'',
            }),
        },
        Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            // `r"…"` / `&"…"` raw and format prefixes.
            prefixes: &["r", "R", "&"],
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
                folds: false,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[],
    doc: DocStyle::Body {
        openers: &["func ", "static func ", "class ", "class_name "],
        delim: "\"\"\"",
    },
    directives: &[],
    regions: &[],
};

// -- dockerfile -------------------------------------------------------------

// A `#` is a comment only as the first non-blank byte of a line; everywhere else
// it is an instruction argument (`RUN echo a#b`). Comments do not continue on a
// trailing backslash in Docker, so `continues_on_backslash: false`.
// `*.Dockerfile` suffixes are handled by `detect`, not by `filenames`.
// UNVERIFIED: `RUN <<EOF` heredocs are declared but `Engine::Generic` ignores
// `heredocs`, so a `#` inside a heredoc body is currently scanned as a comment.
pub static DOCKERFILE: Syntax = Syntax {
    name: "dockerfile",
    extensions: &["dockerfile"],
    filenames: &["Dockerfile", "Containerfile"],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::LineStart,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
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
            text: Some(TextGlyphs {
                escapes: true,
                folds: true,
                delim: b'\'',
            }),
        },
    ],
    heredocs: &[Heredoc {
        openers: &["<<", "<<-"],
        strip_tabs: true,
    }],
    doc: DocStyle::None,
    directives: &[
        "syntax=",
        "escape=",
        "hadolint",
        "checkov:skip",
        "trivy:ignore",
        "tfsec:ignore",
    ],
    regions: &[],
};

// -- makefile ---------------------------------------------------------------

// Make has no string literals: `quotes` is empty by design, and `$(…)`/`$$` are
// expansion syntax, not quoting.
// A comment line continues onto the next line when it ends in a backslash, hence
// `continues_on_backslash: true`.
// UNVERIFIED: a recipe line (tab-prefixed) hands its text to the shell, where a
// `#` is *not* a make comment — e.g. `\t@echo '#build'` or `\t$(shell foo#x)`.
// `At::LineStart` cannot see that, so makefile needs a recipe-aware engine
// (hand-written, like Justfile) rather than `Engine::Generic`.
pub static MAKEFILE: Syntax = Syntax {
    name: "makefile",
    extensions: &["mk", "make"],
    filenames: &["Makefile", "makefile", "GNUmakefile", "BSDmakefile"],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::LineStart,
        continues_on_backslash: true,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// -- cmake ------------------------------------------------------------------

// CMake bracket comments `#[[ … ]]` / `#[=[ … ]=]` are declared only in the
// `[[ ]]` level-0 form; `#` starts a comment wherever a token may start.
// UNVERIFIED: (a) bracket level `[=[`/`[==[` are not expressible; (b) the
// `Generic` engine tries line comments before block comments (SPEC.md §3 step 1
// then 2), so `#[[` is currently eaten as a one-line `#` comment; (c) bracket
// *arguments* `[[ … ]]`/`[=[ … ]=]` are not strings and so are not declared.
pub static CMAKE: Syntax = Syntax {
    name: "cmake",
    extensions: &["cmake"],
    filenames: &["CMakeLists.txt"],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::WordStart,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[BlockComment {
        open: "#[[",
        close: "]]",
        nested: false,
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
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// -- ini --------------------------------------------------------------------

// `#` and `;` are comment introducers at the start of a line in every INI dialect
// worth supporting (GLib/`GKeyFile`, Python `configparser`, `.editorconfig`,
// Desktops and KDE `.ini` — the repo's own `linux/kde/kate/externaltools/*.ini`
// and `linux/common/gtk/gtk-3.0/settings.ini` carry no inline comments at all).
// `LineStart` avoids purging `a#b` inside a value.
// UNVERIFIED: `configparser` also accepts inline comments after whitespace, and
// `gpg`-style files use `#` mid-line; `WordStart` may be the better choice once a
// real counter-example shows up. Values are unquoted, so `quotes` is empty.
pub static INI: Syntax = Syntax {
    name: "ini",
    extensions: &["ini", "cfg", "conf"],
    filenames: &[".editorconfig"],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[
        LineComment {
            marker: "#",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,
        
            owns_indent_block: false,},
        LineComment {
            marker: ";",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,
        
            owns_indent_block: false,},
    ],
    block: &[],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// -- hcl --------------------------------------------------------------------

// HCL has both `#` and `//` line comments plus `/* … */`, all valid anywhere a
// token may start; template strings interpolate `${…}` and `%{…}`.
// UNVERIFIED: `<<EOF` / `<<-EOF` heredocs are declared but `Engine::Generic`
// ignores `heredocs`, so a `#` inside a heredoc body is scanned as a comment.
pub static HCL: Syntax = Syntax {
    name: "hcl",
    extensions: &["tf", "tfvars", "hcl"],
    filenames: &[],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[
        LineComment {
            marker: "#",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
        
            owns_indent_block: false,},
        LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
        
            owns_indent_block: false,},
    ],
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
    heredocs: &[Heredoc {
        openers: &["<<", "<<-"],
        strip_tabs: true,
    }],
    doc: DocStyle::None,
    directives: &["tfsec:ignore", "checkov:skip", "tflint-ignore"],
    regions: &[],
};

// -- starlark (bazel) -------------------------------------------------------

// Bazel/Starlark is Python's comment and string syntax without indentation
// semantics, so `Engine::Generic` (not `Indented`) and no `DocStyle::Body`.
// `//` is not a comment here — it is floor division.
pub static STARLARK: Syntax = Syntax {
    name: "starlark",
    extensions: &["bzl", "bazel"],
    filenames: &[
        "BUILD",
        "BUILD.bazel",
        "WORKSPACE",
        "WORKSPACE.bazel",
        "MODULE.bazel",
        "Tiltfile",
    ],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[],
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
                delim: b'"',
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
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

// -- justfile ---------------------------------------------------------------

// `#` is a comment on a just *statement* line; a recipe body is handed to the
// shell line-by-line, so `#` inside a recipe is the shell's business.
// UNVERIFIED: `At::LineStart` cannot tell a recipe body from a statement, so
// Justfile needs the same recipe-aware (hand-written) engine as Makefile.
// Just strings are `'…'` (raw), `"…"` (escaped) and the `x`/`f` prefixed forms;
// `x'…'`/`x"…"` are not declared.
pub static JUSTFILE: Syntax = Syntax {
    name: "justfile",
    extensions: &[],
    filenames: &["Justfile", "justfile", ".justfile"],
    shebangs: &[],
    family: Family::Hash,
    engine: Engine::Generic,
    line: &[LineComment {
        marker: "#",
        at: At::LineStart,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
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
    directives: &[],
    regions: &[],
};

/// Every `Family::Hash` dialect descriptor, keyed by `Syntax::name`.
pub static HASH: &[(&str, &Syntax)] = &[
    ("python", &PYTHON),
    ("toml", &TOML),
    ("yaml", &YAML),
    ("elixir", &ELIXIR),
    ("crystal", &CRYSTAL),
    ("nim", &NIM),
    ("perl", &PERL),
    ("r", &R),
    ("julia", &JULIA),
    ("powershell", &POWERSHELL),
    ("tcl", &TCL),
    ("fish", &FISH),
    ("awk", &AWK),
    ("gdscript", &GDSCRIPT),
    ("dockerfile", &DOCKERFILE),
    ("makefile", &MAKEFILE),
    ("cmake", &CMAKE),
    ("ini", &INI),
    ("hcl", &HCL),
    ("starlark", &STARLARK),
    ("justfile", &JUSTFILE),
];
