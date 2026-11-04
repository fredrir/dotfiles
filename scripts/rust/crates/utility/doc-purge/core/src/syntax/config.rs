//! JSONC and JSON5.
//!
//! Paste target: `core/src/syntax/config.rs`. Requires `core/src/syntax/mod.rs`
//! per SPEC §1.
//!
//! # Plain `.json` is deliberately NOT claimed
//!
//! A `//` in a `.json` file is a **syntax error**, not a comment. `.json` has no
//! comments, so any `//` or `/* */` a human wrote there is either (a) a bug the
//! author is about to discover, or (b) text inside a string that must not be
//! touched, or (c) a hand-edited file that the next `jq`/parser will reject
//! anyway. Running `doc-purge` over it and deleting the "comments" would be the
//! *wrong repair*: it would silently change a file that some tool is entitled to
//! assume is strict JSON, and it would hide the author's actual error.
//!
//! Therefore:
//!   * `.json` must resolve to **no** `Language` at all — `--uncovered` will
//!     list it, which is the correct signal.
//!   * `.jsonc` and `.json5` are claimed here.
//!   * `tsconfig.json`-style files are *duplicates of* `.jsonc` in spirit but are
//!     not claimed: the extension is `.json`. Per SPEC §2 the only way to claim
//!     one is a `filenames` entry, and claiming `tsconfig.json` would then make
//!     `doc-purge` edit a file whose extension promises strict JSON. Not claimed.
//!
//! # Verified against this repo
//!
//! Six real `.jsonc` files exist and were read:
//! `macos/fastfetch/config.jsonc`, `linux/arch/fastfetch/config.jsonc`,
//! `linux/ubuntu/fastfetch/config.jsonc`, `shared/fastfetch/config.jsonc`,
//! `linux/hyprland/waybar/config.jsonc`, `linux/hyprland/waybar/modules.jsonc`.
//!
//! Findings that shaped the tables below:
//!   * every one uses `//` only — zero `/* */` blocks in the repo, so the block
//!     entry is UNVERIFIED against real input (still required: `.jsonc` permits
//!     it and VS Code emits it);
//!   * `//` appears **after a value on the same line** —
//!     `}, // theme:separator`, `"…" // border_on_canvas #616279` — so
//!     `At::LineStart` would miss most of them. `At::Anywhere` is required;
//!   * `//` also appears **inside a string**:
//!     `"$schema": "https://github.com/…"` (line 2 of all four fastfetch
//!     configs). This is the load-bearing proof that the `"` `Quote` must be
//!     present and must be honoured before a `//` inside the string body is
//!     considered. The generic engine consumes the string in step 3 when it
//!     reaches the `"`, so the `//` bytes are never visited. Correct by
//!     construction — but only because the quote is in the table.
//!   * no single quotes, no trailing-comma weirdness in the sampled files.

use crate::syntax::*;

/// JSON with comments (VS Code dialect: `//` and `/* */`, no trailing commas,
/// no unquoted keys, no single quotes).
pub static JSONC: Syntax = Syntax {
    name: "jsonc",
    extensions: &["jsonc"],
    // `filenames` intentionally empty — see the module doc. `tsconfig.json`,
    // `jsconfig.json` and `.vscode/settings.json` are NOT claimed.
    filenames: &[],
    shebangs: &[],
    family: Family::Dash,
    engine: Engine::Generic,
    line: &[LineComment {
        // Must be `Anywhere`: verified inline trailing `//` in all six repo
        // `.jsonc` files. `At::LineStart` would purge almost nothing.
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
    
    owns_indent_block: false,}],
    block: &[BlockComment {
        open: "/*",
        close: "*/",
        // JSONC does not nest. UNVERIFIED against repo input (no `/* */`
        // present); `false` matches the VS Code implementation.
        nested: false,
        at: At::Anywhere,
        // `/*!` / `/**` are not meaningful in JSONC; a commented-out block is a
        // commented-out block.
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // Only `"` is a string in JSONC. `Escape::Backslash` is right: JSON
        // escapes are `\"`, `\\`, `\uXXXX` — never doubling.
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
        // No `'`: a single-quoted string is invalid JSONC. Omitting it means
        // such a file mis-lexes — which is the honest outcome for invalid input.
    ],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[],
};

/// JSON5 (json5.org): `//` and `/* */`, plus single-quoted strings, unquoted
/// identifier keys, trailing commas, and multi-line strings via `\` + newline.
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
    
    owns_indent_block: false,}],
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
            // JSON5 strings may span lines when a `\` precedes the newline. A
            // raw newline inside is still an error, but `Quote::multiline` is
            // all-or-nothing and `true` is the safer of the two: a terminated
            // multi-line string is lexed correctly, whereas `false` would Bail
            // on a legal file.
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

/// Config-format descriptors, in registry order.
///
/// Note the absence of plain `json` — deliberate, see the module doc. If a
/// `Dialect::Json` variant exists for other reasons (e.g. `dot-code/server`
/// highlighting), its descriptor must have `line: &[]` and `block: &[]` and
/// must never be handed to the purger.
pub static CONFIG: &[(&str, &Syntax)] = &[("jsonc", &JSONC), ("json5", &JSON5)];
