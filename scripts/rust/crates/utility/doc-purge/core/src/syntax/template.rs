//! `core/src/syntax/template.rs` — descriptors for the `Family::Template`
//! dialects.
//!
//! Paste as a sibling of `syntax/mod.rs`; `registry.rs` re-exports it.
//!
//! # Rendered vs silent — read this first
//!
//! Every dialect here has two comment-shaped constructs that look alike and
//! differ in one crucial way: one is *stripped by the template engine* and one
//! is *emitted into the HTML*. Purging the first is a no-op for output; purging
//! the second deletes content from the shipped page. It is never the other way
//! round — nothing in this file is safe to purge just because it starts with a
//! slash.
//!
//! | dialect   | PURGE (silent)      | NEVER PURGE (rendered) |
//! |-----------|---------------------|------------------------|
//! | Pug       | `//- …`             | `// …`                 |
//! | Haml      | `-# …`              | `/ …`, `/[…]`          |
//! | Slim      | `/ …`               | `/! …`                 |
//! | Jinja     | `{# … #}`           | —                      |
//! | Twig      | `{# … #}`           | —                      |
//! | Handlebars| `{{! … }}`, `{{!-- --}}` | —                 |
//! | Mustache  | `{{! … }}`          | —                      |
//! | Liquid    | `{% comment %}…{% endcomment %}`, `{% # … %}` | — |
//! | ERB       | `<%# … %>`          | —                      |
//! | EJS       | `<%# … %>`          | —                      |
//!
//! The three indentation dialects are the dangerous ones, because "purge the
//! comment" and "purge a line that starts with `/`" are *almost* the same
//! predicate. Getting them confused either emits broken markup (purging `//` in
//! Pug removes a `<div>`-level HTML comment and can leave a dangling `-->` if
//! the author wrote it by hand) or leaves comments in the output (declaring
//! `//-` and `//` both silent).
//!
//! # `Engine::Verbatim` is used here and does not exist in SPEC §1 yet
//!
//! `{{ … }}` and `{% … %}` bodies contain **no comments of any kind** in every
//! dialect below. The only correct behaviour is "scan to the close delimiter and
//! emit nothing". `Engine::Verbatim` (see `syntax-markup.rs`) is that engine;
//! substituting `Engine::Generic` or the host engine re-scans the expression
//! body with this descriptor's own `block`/`line` tables, which would let
//! `{% set x = "{# not a comment #}" %}` lose a chunk of a string literal.
//!
//! # Longest open wins, across comments *and* regions
//!
//! `{{!--` > `{{!` > `{{`, and `{% verbatim %}` > `{%`, and `{%#` > `{%`,
//! `<%#` > `<%`. A first-match scan either truncates the purge span or leaks the
//! comment tail.
//!
//! # Whitespace-control markers
//!
//! Jinja/Twig/Liquid/Nunjucks write `{#- … -#}`, `{%- … -%}`, `{{- … -}}` and
//! `#}`, `%}`, `}}` may be preceded by `-`. `BlockComment` bodies below are
//! written so the `-` lands *inside* the body (`open: "{#"`, `close: "#}"`), so
//! the purge span automatically covers `{#- … -#}`. **Regions are the problem**
//! (see finding A in `markup-data.md`): `open: "{{"` / `close: "}}"` matches the
//! untrimmed form, and `{{- x -}}` still matches (the `-` is inside the body),
//! but a `close` that arrives as `-}}` is found at the `}}` — fine. What breaks
//! is `{%- comment -%}`-style *tag names*, `{%- raw -%}`, where a literal
//! `{%- raw` open is needed. Recommended: `Syntax.trim_markers: &'static [u8]`
//! (here `&[b'-']`) so the region matcher may skip one trim byte after the
//! delimiter and before it in the close.

use crate::syntax::*;

// ---------------------------------------------------------------------------
// Jinja
// ---------------------------------------------------------------------------

/// `.jinja` / `.jinja2` / `.j2`.
///
/// `{# … #}` is a `BlockComment`; `{{ … }}` and `{% … %}` are verbatim regions.
///
/// Jinja has **no** comment form inside `{{ }}` or `{% %}` — `{# }` is the only
/// comment, and it cannot be nested (Jinja's lexer does not track depth).
/// `to_eof_ok: false`: an unterminated `{#` is a template error, so `Bail` is
/// right.
///
/// Python-style `#` must NOT be declared: `{#` is the comment, and a bare `#`
/// inside a template is literal text (`{#1 in the chart}` is prose).
pub static JINJA: Syntax = Syntax {
    name: "jinja",
    extensions: &["jinja", "jinja2", "j2"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[],
    block: &[BlockComment {
        open: "{#",
        close: "#}",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore", "noqa"],
    regions: &[
        // `{% raw %}…{% endraw %}`: everything between is literal output, so it
        // must not be lexed at all. Must be listed (and matched) before `{%`.
        Region {
            open: "{% raw %}",
            close: "{% endraw %}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{{",
            close: "}}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{%",
            close: "%}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Twig
// ---------------------------------------------------------------------------

/// `.twig`.
///
/// Same three delimiters as Jinja. Two Twig-only additions:
///
/// * `{% verbatim %} … {% endverbatim %}` — Twig's `raw` block, verbatim.
/// * `{% comment %} … {% endcomment %}` — Twig *does* have a block comment tag.
///   It is a `BlockComment` (Twig does not nest it), listed after `{% verbatim %}`
///   so the longer tag name wins.
///
/// `{% endcomment %}` vs `{% comment %}`: the pair must be matched as a pair.
/// Listing `{% comment %}` as a `BlockComment` open with `close: "{% endcomment %}"`
/// is exactly that. Do **not** also let `{%` open a region first.
///
/// UNVERIFIED: whether Twig's `{% comment %}` may contain a `%}`. The docs do
/// not say; it does not matter for the purge span (we scan for
/// `{% endcomment %}`), but it does mean the body is treated as text, which is
/// correct.
pub static TWIG: Syntax = Syntax {
    name: "twig",
    extensions: &["twig"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[],
    block: &[
        BlockComment {
            open: "{% comment %}",
            close: "{% endcomment %}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{#",
            close: "#}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore"],
    regions: &[
        Region {
            open: "{% verbatim %}",
            close: "{% endverbatim %}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{{",
            close: "}}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{%",
            close: "%}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Handlebars
// ---------------------------------------------------------------------------

/// `.hbs` / `.handlebars`.
///
/// Two comment forms, and the difference between them is load-bearing:
///
/// * `{{! … }}` — short form. Ends at the first `}}`, so it **cannot** contain
///   `}}` in its body.
/// * `{{!-- … --}}` — long form, exists *precisely* so the body may contain
///   `}}` (e.g. `{{!-- renders {{foo}} --}}`). Its close is `--}}`.
///
/// If `{{!` is matched before `{{!--`, the short form's `}}` close fires inside
/// the long form and the *tail* of `{{!-- renders {{foo}} --}}` is emitted as
/// template code. `--` before `}}` is what makes the long form unambiguous, and
/// it means longest-open-first ordering is mandatory: `{{!--` (5 bytes) before
/// `{{!` (3) before `{{` (2).
///
/// `{{{{raw}}}} … {{{{/raw}}}}` (raw block) is declared verbatim below;
/// UNVERIFIED that it is common enough to matter, but it costs one entry.
pub static HANDLEBARS: Syntax = Syntax {
    name: "handlebars",
    extensions: &["hbs", "handlebars"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[],
    block: &[
        BlockComment {
            open: "{{!--",
            close: "--}}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{{!",
            close: "}}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore"],
    regions: &[
        Region {
            open: "{{{{raw}}}}",
            close: "{{{{/raw}}}}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{{",
            close: "}}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Mustache
// ---------------------------------------------------------------------------

/// `.mustache`.
///
/// The Mustache spec has only the short `{{! … }}` form. The long `{{!-- --}}`
/// form is a Handlebars extension that Mustache implementations in the wild
/// (and every editor) accept, so it is declared too — with the same ordering
/// rule as `HANDLEBARS`.
///
/// Mustache has no other comment syntax: `{{# section}}` is a section, `{{^ }}`
/// an inverted section, `{{/ }}` a close. **Do not add `{{#` or `{{/` to
/// `block`.**
pub static MUSTACHE: Syntax = Syntax {
    name: "mustache",
    extensions: &["mustache"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[],
    block: &[
        BlockComment {
            open: "{{!--",
            close: "--}}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{{!",
            close: "}}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[Region {
        open: "{{",
        close: "}}",
        engine: Engine::Verbatim,
        needs_attr: None,

        close_at: At::Anywhere,
        once: false,
    }],
};

// ---------------------------------------------------------------------------
// Liquid
// ---------------------------------------------------------------------------

/// `.liquid`.
///
/// Three comment forms:
///
/// 1. `{% comment %} … {% endcomment %}` — the classic block. Does **not**
///    nest (Liquid's `comment` tag is not depth-tracking), so `nested: false`
///    and the first `{% endcomment %}` wins.
/// 2. `{% # … %}` — the inline comment tag (Liquid ≥ 5.4). It is a
///    `BlockComment` with `open: "{% #"`, **not** a `LineComment`, because its
///    terminator is `%}`, not a newline. `{% # x\n   y %}` is one comment.
/// 3. `{%- # … -%}` — the whitespace-controlled spelling. `open: "{%- #"` /
///    `"{%-#"` entries are listed; see finding A (a `trim_markers` field would
///    collapse all four spellings into one).
///
/// `{{ … }}` / `{% … %}` are verbatim regions, and `{% comment %}` must be
/// matched before `{%` — a 13-byte open beating a 2-byte one.
///
/// Trap: Liquid's `{% raw %} … {% endraw %}` blocks contain literal `{{ }}`
/// that must not be treated as expressions — declared verbatim below.
///
/// Trap: `{{ '{% comment %}' }}` — a rendered string. The verbatim `{{ }}`
/// region wins because it opens earlier. Correct order matters.
pub static LIQUID: Syntax = Syntax {
    name: "liquid",
    extensions: &["liquid"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[],
    block: &[
        BlockComment {
            open: "{% comment %}",
            close: "{% endcomment %}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{%- comment -%}",
            close: "{%- endcomment -%}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{% #",
            close: "%}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{%#",
            close: "%}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{%- #",
            close: "%}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{%-#",
            close: "%}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore"],
    regions: &[
        Region {
            open: "{% raw %}",
            close: "{% endraw %}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{%- raw -%}",
            close: "{%- endraw -%}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{{",
            close: "}}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "{%",
            close: "%}",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// ERB
// ---------------------------------------------------------------------------

/// `.erb` / `.rhtml` (and `.html.erb`, which resolves by last extension).
///
/// Host is HTML, so `Engine::Markup` + `Family::Template`: markup comment rules
/// outside the tags (`<!-- -->` is a real HTML comment in the emitted page —
/// rendered, but it is a comment, which is the tool's business), Ruby inside
/// `<% … %>` / `<%= … %>`.
///
/// `<%# … %>` is the silent EEx-style comment and is matched before `<%`
/// (longest open first).
///
/// # Ruby's `#` versus the `%>` terminator — a real conflict
///
/// `<% x = 1 # a note %>` is legal ERB: the `#` opens a Ruby comment, and ERB
/// still closes the tag at `%>`. A `Engine::Ruby` region body lexed naively will
/// swallow `%>` into the line comment and then `Bail("unterminated region")`.
///
/// Required rule, which SPEC §4 does not state: **inside a region, `Region.close`
/// takes precedence over a line comment that would swallow it.** Concretely the
/// Ruby engine must, when it opens a `#` comment inside a region, stop the
/// comment at the region's `close` offset if that comes first on the same line.
/// Without it every ERB template with an inline Ruby comment is `Skipped`.
/// Marked UNVERIFIED in the report; there is no descriptor field that can say it.
///
/// # `%>` inside a Ruby string
///
/// `<%= "100%> done" %>` — the same precedence requirement in the other
/// direction: the close must be found *through* the Ruby engine's string state,
/// not by a raw `%>` byte scan.
///
/// `<%%` is the escape for a literal `<%` and must not open a region.
pub static ERB: Syntax = Syntax {
    name: "erb",
    extensions: &["erb", "rhtml"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Markup,
    line: &[],
    block: &[
        BlockComment {
            open: "<%#",
            close: "%>",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "<!--",
            close: "-->",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore", "erb-lint:", "rubocop:"],
    regions: &[
        Region {
            open: "<%=",
            close: "%>",
            engine: Engine::Ruby,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<%-",
            close: "-%>",
            engine: Engine::Ruby,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<%",
            close: "%>",
            engine: Engine::Ruby,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// EJS
// ---------------------------------------------------------------------------

/// `.ejs`.
///
/// Same shape as ERB with JavaScript instead of Ruby, so `Engine::Script` and
/// the canonical-embedded-descriptor rule (REGION GAP 4) applies: the region's
/// engine brings `//` and `/* */` with it, and this descriptor must not declare
/// them — `http://` in the HTML body of an EJS file would be eaten otherwise.
///
/// `<%# … %>` is the EJS comment and is matched before `<%`.
/// `<%-` / `-%>` is the unescaped-output form.
///
/// Note `<%/* … */%>` also works in EJS (a JS block comment inside a tag); it is
/// handled by the region's JS engine, not by this descriptor.
pub static EJS: Syntax = Syntax {
    name: "ejs",
    extensions: &["ejs"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Markup,
    line: &[],
    block: &[
        BlockComment {
            open: "<%#",
            close: "%>",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "<!--",
            close: "-->",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore", "eslint-disable", "c8 ignore"],
    regions: &[
        Region {
            open: "<%=",
            close: "%>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<%-",
            close: "-%>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<%",
            close: "%>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Pug
// ---------------------------------------------------------------------------

/// `.pug` / `.jade`.
///
/// # THE TRAP: `//-` is silent, `//` is RENDERED HTML
///
/// * `//- …` — buffered nowhere, stripped at compile time. Safe to purge.
/// * `// …` — **becomes `<!-- … -->` in the output document.** Purging it
///   deletes a comment the author deliberately shipped, and its indented block
///   usually contains the closing context of the comment body: purging the first
///   line of
///
///   ```text
///   // browser-support note
///     IE11 is unsupported
///   ```
///
///   leaves `IE11 is unsupported` as a stray *text node* in the page — broken
///   markup, not a removed comment.
///
/// So `//` is deliberately **absent** from `line`. There is no `keep_line` table
/// in SPEC §1; the correctness here comes from omission, which is fragile —
/// someone "completing" the dialect by adding `//` silently breaks every Pug
/// file that uses an HTML comment. That is finding B in `markup-data.md`
/// (`keep_line: &'static [&'static str]` makes the intent explicit and
/// order-independent).
///
/// # Indented block comments — REGION GAP 5
///
/// `//- …` also consumes the whole **indented block** beneath it, to the first
/// less-indented line. A line-based purge leaves the block body behind as text.
/// `LineComment` has no "opens an indentation block" bit; the recommendation is
/// an indented-block concept shared by Pug/Haml/Slim (see `markup-data.md`).
///
/// # Other Pug constructs that are not comments
///
/// * `- code` — unbuffered JS. Comments inside it are `//`, and they are on the
///   same *line* as the `-` prefix. Not modelled: under-purge.
/// * `= expr`, `!= expr`, `#{…}`, `!{…}` — expressions.
/// * `script.` / `style.` — indented blocks of JS/CSS. Not modelled (would need
///   an indentation-delimited region, REGION GAP 5); under-purge.
/// * `:markdown` / `:coffee` — filters, same gap.
///
/// `At::LineStart` on `//-` is required, not cosmetic: a `/`-leading marker at
/// `At::Anywhere` would fire on every `a(href='/x')` attribute value.
pub static PUG: Syntax = Syntax {
    name: "pug",
    extensions: &["pug", "jade"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[LineComment {
        marker: "//-",
        at: At::LineStart,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[],
    // Deliberately empty: `'`/`"` appear in prose (`p It's fine`) and a quote
    // opened at any byte would swallow the document. Expression strings are the
    // Indented engine's business, inside `#{ }` / `-` lines only.
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// Haml
// ---------------------------------------------------------------------------

/// `.haml`.
///
/// # THE TRAP: `-#` is silent, `/` is RENDERED
///
/// * `-# …` — silent, stripped. Purge.
/// * `/ …` — a **rendered HTML comment** (`<!-- … -->` in the output). Keep.
/// * `/[…]` — a conditional comment (`/[if IE]`), also rendered. Keep.
///
/// `/` is therefore absent from `line`, same fragility as Pug (`keep_line`
/// finding B).
///
/// # Indented blocks — REGION GAP 5
///
/// `-#` consumes its indented block, exactly like Pug's `//-`.
///
/// # Filters are indented regions
///
/// `:javascript`, `:css`, `:ruby`, `:markdown`, `:erb` introduce an indented
/// block whose body is another language. `//` comments inside `:javascript` are
/// missed (under-purge) because `Region` can only be closed by a marker, never
/// by a dedent. Same fix as REGION GAP 5.
///
/// # What is deliberately not declared
///
/// * `-` — a silent *Ruby statement* line, not a comment. `- # note` embeds a
///   Ruby comment inside it; not modelled (under-purge).
/// * `&=` / `!=` / `=` — expression output.
/// * `!!! 5` — doctype.
///
/// `At::LineStart`: Haml tags take `%a{href: "/x"}` attribute hashes, and a `/`
/// at `At::Anywhere` would eat them.
pub static HAML: Syntax = Syntax {
    name: "haml",
    extensions: &["haml"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[LineComment {
        marker: "-#",
        at: At::LineStart,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore", "rubocop:"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// Slim
// ---------------------------------------------------------------------------

/// `.slim`.
///
/// # THE TRAP, and the one place omission is *not* enough
///
/// * `/ …` — a **silent** Slim comment (`/` at line start). Purge.
/// * `/! …` — an **HTML comment**: emits `<!-- … -->`. Keep.
/// * `/[…]` — a conditional comment (`/[if IE]`). Keep.
/// * `/` followed by an indented block — silent block comment. Purge.
///
/// `/!` and `/` share a prefix **at the same position**, so unlike Pug (`//` vs
/// `//-`) and Haml (`/` vs `-#`), leaving the rendered form out of the tables
/// does not help: declaring `/` at `At::LineStart` matches `/! foo` too and
/// purges it. The `!` must actively veto the `/`.
///
/// SPEC §1 has no keep-prefix table, so this descriptor uses the mechanism that
/// does exist: `directives: &["!"]`. `keep::kept` lowercases the body once and
/// checks the language `directives` at the trimmed body start, so `! Foo` in a
/// `/! Foo` line matches `"!"` and the comment is retained. It works, and it is
/// **not** how this should be expressed — it depends on `directives` being
/// trimmed-body-start (only `GLOBAL_OPENINGS` is documented that way) and it
/// would also retain `/ !!anything`. Recommended instead:
///
/// ```ignore
/// /// Line prefixes that are never comments even though a comment starts here.
/// pub keep_line: &'static [&'static str],   // Slim: &["/!", "/["]
/// ```
///
/// `At::LineStart` is mandatory: Slim writes `a href="/x"` with a leading
/// slash in the value.
///
/// Note `doctype` (`doctype html`) is not a comment.
pub static SLIM: Syntax = Syntax {
    name: "slim",
    extensions: &["slim"],
    filenames: &[],
    shebangs: &[],
    family: Family::Template,
    engine: Engine::Template,
    line: &[LineComment {
        marker: "/",
        at: At::LineStart,
        continues_on_backslash: false,
        doc: false,

        owns_indent_block: false,
    }],
    block: &[],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    // Stand-in for `keep_line: &["/!", "/["]` — see the doc comment. `"!"`
    // retains `/! …` (rendered HTML comment); `"/"` in the list is a no-op
    // because `directives` only sees the body after the marker.
    directives: &["!", "prettier-ignore"],
    regions: &[],
};

/// Registry table for the template dialects. `family` is `Template` for all ten
/// (it is the file's organising family), but `engine` is not uniform:
/// delimiter-hosted dialects use `Engine::Template`, HTML-hosted ones use
/// `Engine::Markup`, and the three indentation dialects use `Engine::Indented`.
/// `Engine` drives the scanner; `Family` only feeds `purge.rs`'s
/// `merging()`/`scripty()`/`line_break()` decisions, which need a
/// `Family::Template` branch of their own (SPEC §5 does not give one).
pub static TEMPLATE: &[(&str, &Syntax)] = &[
    ("jinja", &JINJA),
    ("twig", &TWIG),
    ("handlebars", &HANDLEBARS),
    ("mustache", &MUSTACHE),
    ("liquid", &LIQUID),
    ("erb", &ERB),
    ("pug", &PUG),
    ("haml", &HAML),
    ("slim", &SLIM),
    ("ejs", &EJS),
];
