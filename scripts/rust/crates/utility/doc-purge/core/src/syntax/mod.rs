//! The language descriptor.
//!
//! Every scanner is a pure function of `(&[u8], &'static Syntax)`: it reads the
//! descriptor and never matches on `Dialect`. The descriptor is plain data —
//! `&'static` throughout, no allocation, no dependencies — so a language table
//! is a list of `const`-constructible statics and a new `Dialect` is a compile
//! error until it has one.

/// How a language groups with the languages it is lexed like.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Curly,
    Hash,
    Dash,
    Css,
    Lisp,
    Markup,
    Template,
}

/// Which engine executes this descriptor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    /// `engine/generic.rs` — table-driven, the workhorse.
    Generic,
    /// `curly.rs` — the descriptor-driven C-family scanner. It exists because a
    /// descriptor cannot say "`'` is a char literal only when a `\` or a
    /// closing `'` follows" (Rust lifetimes), nor reproduce C++ raw-tag and
    /// C# verbatim prefixes byte-for-byte. Everything else about it is table.
    Curly,
    /// `engine/dash.rs` — Lua long brackets, the Haskell `--` operator rule and
    /// SQL dollar quoting. One engine, three descriptors, because each needs a
    /// rule a table cannot carry.
    Dash,
    /// `js.rs` — regex/division, JSX, TS generics.
    Script,
    /// `css.rs` — interpolation, custom properties, indented comments.
    Style,
    /// `engine/indented.rs` — dedent blocks + docstrings (Python, GDScript, YAML).
    Indented,
    /// `engine/shell.rs` — heredoc queue, `$( )`, backticks.
    Shell,
    /// `engine/ruby.rs` — `%`-literals, `=begin`, `__END__`.
    Ruby,
    /// `engine/perl.rs` — POD, quote-like operators, heredocs, `__END__`.
    Perl,
    /// `engine/markup.rs` — region stack, embedded engines.
    Markup,
    /// `engine/template.rs` — host + `{{ }}` / `{% %}` / `{# #}` regions.
    Template,
    /// Advance to the enclosing region's close, emitting nothing at all.
    /// Markdown fenced code blocks, XML CDATA, Jinja `{% raw %}`, Liquid
    /// `{% verbatim %}`.
    Verbatim,
    /// The Elixir descriptor under the generic lexer. HEEx/EEX `<%= … %>` bodies.
    /// Not a distinct lexer: dispatch resolves `Dialect::Elixir.syntax()` and runs
    /// `Engine::Generic` over it.
    Elixir,
}

/// Where a comment marker is allowed to start.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum At {
    /// `"//"`, `"/*"`.
    Anywhere,
    /// Dockerfile `#`, Makefile `#` in a rule, VimL `"`.
    LineStart,
    /// Shell `#` (needs a fresh word), SQL `/*! ` variants.
    WordStart,
    /// Only at byte 0 (after a BOM and an optional shebang). Astro frontmatter.
    StartOfFile,
}

/// A line comment marker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineComment {
    pub marker: &'static str,
    pub at: At,
    /// C/C++: a trailing backslash continues the comment onto the next line.
    pub continues_on_backslash: bool,
    /// A run of this marker is a *doc* comment (`///`, `//!`, `##`, `#'`).
    /// Purely informational: it is still purged. Used by `DocStyle` and tests.
    pub doc: bool,
    /// The comment owns the indented block beneath it: Pug `//-`, Haml `-#`,
    /// Slim `/`, Sass `//`. The whole block is the comment body.
    pub owns_indent_block: bool,
}

/// A block comment delimiter pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockComment {
    pub open: &'static str,
    pub close: &'static str,
    pub nested: bool,
    pub at: At,
    /// `/*!` and `/**` keep: the `!` and `*` after `open`. `None` for no such form.
    pub important: Option<char>,
    /// Stylus: an unterminated block legitimately runs to EOF.
    pub to_eof_ok: bool,
}

/// How a quote escapes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Escape {
    /// A backslash eats the next character.
    Backslash,
    /// Nothing is special.
    None,
    /// The close delimiter doubled continues the string.
    Doubling,
}

/// A string literal form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quote {
    /// Opening delimiter as written after any prefix: `"`, `'`, "```", "\"\"\"".
    pub open: &'static str,
    /// Closing delimiter; `""` means "same as open".
    pub close: &'static str,
    pub escape: Escape,
    pub multiline: bool,
    /// Prefix bytes that may precede the delimiter: Python `r b u f rb br`,
    /// Rust `r b br`, C# `@ $ @$`, Swift `#`. Matched only when the quote is the
    /// next byte.
    pub prefixes: &'static [&'static str],
    /// Rust/Swift: `r#"…"#` closes on `"` followed by N hashes. `false` otherwise.
    pub hashes: bool,
    /// C++ `R"tag(… )tag"`. `false` otherwise.
    pub raw_tag: bool,
    /// Feed `Out.text` so glyph sweeping can run: (escapes, folds, delimiter).
    /// `None` means "record the string but do not sweep glyphs" (e.g. regex-ish).
    pub text: Option<TextGlyphs>,
}

/// What a string body contributes to glyph sweeping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextGlyphs {
    pub escapes: bool,
    pub folds: bool,
    pub delim: u8,
}

/// A heredoc form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Heredoc {
    /// `["<<", "<<-", "<<~"]`.
    pub openers: &'static [&'static str],
    pub strip_tabs: bool,
}

/// How this language expresses documentation in *string literals*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocStyle {
    None,
    /// A string that is the sole statement of an indented body under an opener
    /// ending in `:`, where the opener starts with one of `openers`.
    Body {
        openers: &'static [&'static str],
        delim: &'static str,
    },
    /// Elixir `@moduledoc "…"`, `@doc "…"`, `@typedoc "…"`.
    Attribute {
        attributes: &'static [&'static str],
    },
    /// Julia: a string literal immediately above a `def`/`function`/`struct`.
    Before {
        keywords: &'static [&'static str],
    },
    /// Clojure/Common Lisp: a string literal immediately after the name in a form.
    AfterName,
    /// Lua `---@param x t description` — trailing prose trimmed by `luadoc`.
    Ldoc,
    /// Nim `##`, R roxygen `#'`: doc comments, no body rewrite.
    CommentOnly,
}

/// A region inside a markup file that must be lexed by another engine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Region {
    /// Opening tag or delimiter, e.g. `"<script"`, `"<style"`, `"{{-"`, `"<%"`.
    pub open: &'static str,
    pub close: &'static str,
    /// Which engine lexes the region body.
    pub engine: Engine,
    /// Only for markup tags: require `attr` to be present in the opening tag.
    pub needs_attr: Option<(&'static str, &'static str)>,
    /// Where the closing marker is allowed to match. Default in the tables is
    /// `At::Anywhere`. Astro's frontmatter closes on a `---` at line start.
    pub close_at: At,
    /// Consume at most one such region per file. Astro frontmatter.
    pub once: bool,
}

/// Everything a scanner is allowed to know about a language.
///
/// Empty slice convention: `line: &[]`, `block: &[]`, `quotes: &[]`,
/// `heredocs: &[]`, `regions: &[]`, `filenames: &[]`, `shebangs: &[]`,
/// `directives: &[]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Syntax {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// Extensionless files: "Makefile", "Dockerfile", "CMakeLists.txt", "Gemfile".
    pub filenames: &'static [&'static str],
    /// Shebang interpreters: "sh", "bash", "python", "node", "lua".
    pub shebangs: &'static [&'static str],
    pub family: Family,
    pub engine: Engine,
    pub line: &'static [LineComment],
    pub block: &'static [BlockComment],
    pub quotes: &'static [Quote],
    /// Empty = no heredocs.
    pub heredocs: &'static [Heredoc],
    pub doc: DocStyle,
    /// Extra directive markers this language keeps, checked by `keep.rs`
    /// against the comment body, case-insensitively, `starts_with`.
    pub directives: &'static [&'static str],
    /// Markup/template regions; empty otherwise.
    pub regions: &'static [Region],
}

pub mod config;
pub mod curly;
pub mod dash;
pub mod hash;
pub mod markup;
pub mod script;
pub mod shell_ruby;
pub mod style;
pub mod template;
