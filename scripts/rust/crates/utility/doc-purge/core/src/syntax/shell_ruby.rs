//! Syntax descriptors for the shell and Ruby families (`Family::Hash`).
//!
//! Both dialects have a `#` line comment, but at different places: a shell `#`
//! starts a comment only at a word boundary (`echo ${#name}`, `a#b` are not
//! comments), while Ruby's `#` comments anywhere. Ruby also has the `=begin`
//! / `=end` block form at the start of a line.
//!
//! `Engine::Shell` and `Engine::Ruby` supply the state the table cannot
//! describe: `$(…)`, `${…}`, `((…))`, heredoc queues and backticks for the
//! shell; `%`-literals, `/regex/`, `?c` character literals and `__END__` for
//! Ruby. The quote entries below are the plain forms those engines also accept.

use crate::syntax::*;

/// Shell: `#` at `At::WordStart`, not `Anywhere`. Evidence for the choice is
/// in `hash.rs`: `$#`, `"${#name}"`, `${reply#*:}` and `(( #c1 ))` are code,
/// not comments, so `At::Anywhere` would truncate them and `At::LineStart`
/// would miss `echo hi # note`.
pub static SHELL: Syntax = Syntax {
    name: "shell",
    extensions: &["sh", "bash", "zsh", "ksh"],
    filenames: &[
        ".bashrc",
        ".zshrc",
        ".profile",
        ".bash_profile",
        ".bash_logout",
        "PKGBUILD",
        ".zshenv",
        ".zprofile",
    ],
    shebangs: &["sh", "bash", "zsh", "ksh", "dash", "ash"],
    family: Family::Hash,
    engine: Engine::Shell,
    line: &[LineComment {
        marker: "#",
        at: At::WordStart,
        continues_on_backslash: false,
        doc: false,
        owns_indent_block: false,
    }],
    block: &[],
    quotes: &[
        // "…" — backslash escapes, may span lines.
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
        // '…' — no escapes in `hash.rs::Shell::single`; may span lines.
        Quote {
            open: "'",
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
        // `…` — command substitution. `Shell` scans it as a backslash-escaped
        // run and does not record glyph text for the body, so `text` is `None`.
        Quote {
            open: "`",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        },
    ],
    // `hash.rs::Shell::heredoc_head` accepts `<<` and `<<-`; `take_heredocs`
    // strips leading tabs for the `-` form.
    heredocs: &[Heredoc {
        openers: &["<<", "<<-"],
        strip_tabs: true,
    }],
    doc: DocStyle::None,
    directives: &["shellcheck", "shfmt:", "bats", "shellspec"],
    regions: &[],
};

/// Ruby: `#` anywhere, `=begin` / `=end` at column 0, and three quote forms.
/// Engine note: `At::LineStart` is the closest predicate the descriptor has,
/// but it also accepts an indented `=begin`; `hash.rs::Ruby` still enforces the
/// column-zero rule.
pub static RUBY: Syntax = Syntax {
    name: "ruby",
    extensions: &["rb", "rake", "gemspec", "ru", "podspec", "jbuilder", "thor"],
    filenames: &[
        "Gemfile",
        "Rakefile",
        "Brewfile",
        "Vagrantfile",
        "Guardfile",
        "Podfile",
        "Fastfile",
    ],
    shebangs: &["ruby"],
    family: Family::Hash,
    engine: Engine::Ruby,
    line: &[LineComment {
        marker: "#",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
        owns_indent_block: false,
    }],
    block: &[BlockComment {
        open: "=begin",
        close: "=end",
        nested: false,
        at: At::LineStart,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[
        // "…" and '…' really differ in Ruby, but `hash.rs::Ruby::plain` scans
        // both with backslash escapes; the descriptor follows the scanner.
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
        // `…` — command execution, scanned like a double-quoted string.
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
    // `hash.rs::Ruby::heredoc_head` strips for both `<<-` and `<<~`.
    heredocs: &[Heredoc {
        openers: &["<<", "<<-", "<<~"],
        strip_tabs: true,
    }],
    doc: DocStyle::None,
    directives: &[
        "rubocop:",
        ":nodoc:",
        ":nocov:",
        "frozen_string_literal:",
        "encoding:",
        "typed:",
        "steep:",
    ],
    regions: &[],
};

/// Shell and Ruby descriptors, in registry order.
pub static SHELL_RUBY: &[(&str, &Syntax)] = &[("shell", &SHELL), ("ruby", &RUBY)];
