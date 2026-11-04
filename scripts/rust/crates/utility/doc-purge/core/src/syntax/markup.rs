//! `core/src/syntax/markup.rs` — descriptors for the `Family::Markup` dialects.
//!
//! Paste as a sibling of `syntax/mod.rs`; `registry.rs` re-exports it.
//!
//! # Read this before integrating
//!
//! ## `Engine::Verbatim` is used here and does not exist in SPEC §1 yet
//!
//! Every `engine: Engine::Verbatim` below is a *correctness requirement*, not a
//! convenience: it is the only way to say "advance to `Region.close` and emit
//! nothing at all". SPEC §4's `Engine` has no such variant, so the closest
//! substitute (`Engine::Generic`, or the host engine) re-scans the region body
//! with *this* descriptor's marker tables and will happily purge `<!--` inside a
//! fenced code block, a CDATA section or an MDX expression. Add to
//! `syntax/mod.rs`:
//!
//! ```ignore
//! Verbatim,  // engine/verbatim.rs — scan to Region.close, emit no comments
//! ```
//!
//! ## Quotes are deliberately empty for every HTML/XML-shaped dialect
//!
//! `engine/markup.rs` parses tags itself (exactly as `js.rs::element()` at
//! js.rs:798 already does) and must skip attribute values in that tag scanner.
//! Declaring `Quote` entries for `"`/`'` here would be *active harm*: the
//! generic byte loop opens a quote at any byte, so the first `don't` or
//! `it's` in prose opens a string and the rest of the document is swallowed or
//! handed back as `Bail("unterminated string")`.
//!
//! ## Marker matching must be longest-first across comments *and* regions
//!
//! `<%#` before `<%`, `{% verbatim %}` before `{%`, `{{!--` before `{{!`,
//! `{/*` before `/*`. A first-match loop gets the purge span wrong or leaks the
//! tail of the comment into the code.
//!
//! ## `<script` / `<style` openers need a following-byte check
//!
//! `<script` is a prefix of `<scriptfoo`. The markup engine must require the
//! next byte after `Region.open` to not be a name byte (and, for HTML, to be
//! whitespace, `>`, or `/`).

use crate::syntax::*;

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

/// `.html` / `.htm`.
///
/// Regions: `<script>` is JavaScript, `<style>` is CSS. Both engines bring
/// their own canonical descriptor (see the `Engine::Script` note in
/// `markup-data.md`); this descriptor carries *no* `//` because a bare
/// `//` in an HTML text node is a URL (`See http://example.com`), not a
/// comment. Putting `//` in `line` here would eat every bare URL on the page.
pub static HTML: Syntax = Syntax {
    name: "html",
    extensions: &["html", "htm"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "prettier-ignore",
        "cspell:disable",
        "cspell:enable",
        "htmlhint",
    ],
    regions: &[
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// XHTML
// ---------------------------------------------------------------------------

/// `.xhtml` / `.xht`.
///
/// XML-serialised HTML: the `<!-- -->` rule is XML's (see `XML`), but the
/// embedded `<script>`/`<style>` bodies are still text/javascript and text/css,
/// so the regions survive. XHTML is normally served as `application/xhtml+xml`
/// and therefore *cannot* contain `<![CDATA[ ]]>`-free script bodies — the
/// CDATA wrapper is common; see the CDATA note in `XML`.
pub static XHTML: Syntax = Syntax {
    name: "xhtml",
    extensions: &["xhtml", "xht"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore", "cspell:disable", "cspell:enable"],
    regions: &[
        Region {
            open: "<![CDATA[",
            close: "]]>",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

/// `.xml` / `.xsd` / `.rng` / `.wsdl` / `.xul` / flat ODF / `.xaml`.
///
/// `--` is **illegal inside an XML comment** (XML 1.0 §2.5). Two consequences
/// the engine must respect:
///
/// 1. `-->` is the only terminator. `--!>` (which HTML5 accepts) is not.
/// 2. A `--` in a comment body is a *document* error, not a terminator. Do not
///    "helpfully" close on a bare `--`; that would truncate the purge span and
///    emit the tail of the comment as markup.
///
/// The practical reading is therefore "first `-->` wins", which is what a
/// single non-nesting `BlockComment` gives us. `to_eof_ok: false` because XML
/// has no construct where an unterminated comment is legal — an unterminated
/// `<!--` means the file is broken and we should `Bail`, not guess.
///
/// CDATA (`<![CDATA[ … ]]>`) is **not** a comment and must never be purged, but
/// it *can* contain the byte sequence `<!--`. Getting this wrong both stops a
/// real comment from being purged and can start a phantom one. That is the
/// strongest single argument for `Engine::Verbatim` + a CDATA `Region` here:
///
/// ```ignore
/// Region { open: "<![CDATA[", close: "]]>", engine: Engine::Verbatim, needs_attr: None }
/// ```
///
/// It is commented out rather than live because a live region would change the
/// CDATA body from "scanned by the host" to "scanned by nothing", and the
/// difference is only safe once `Engine::Verbatim` exists.
pub static XML: Syntax = Syntax {
    name: "xml",
    extensions: &[
        "xml", "xsd", "rng", "wsdl", "xul", "fodt", "fods", "xaml", "resx",
    ],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore", "cspell:disable", "cspell:enable"],
    regions: &[Region {
        open: "<![CDATA[",
        close: "]]>",
        engine: Engine::Verbatim,
        needs_attr: None,
        close_at: At::Anywhere,
        once: false,
    }],
};

// ---------------------------------------------------------------------------
// SVG
// ---------------------------------------------------------------------------

/// `.svg` only. **`.svgz` is not claimed: it is gzip-compressed** and would
/// decode to binary garbage. Detection should treat `.svgz` as uncovered
/// (`--uncovered` will list it) until there is a gzip path in the CLI, which is
/// out of scope for 2.0.
///
/// SVG is XML, so the `--`-in-comment rule applies verbatim. `<script>` and
/// `<style>` are real in SVG (SMIL-less scripting is common in exported
/// diagrams), so the regions are kept.
pub static SVG: Syntax = Syntax {
    name: "svg",
    extensions: &["svg"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore"],
    regions: &[
        Region {
            open: "<![CDATA[",
            close: "]]>",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// `.md` / `.markdown` / …
///
/// # `#` IS A HEADING, NOT A COMMENT
///
/// If somebody ever adds `#` to `line` here, every heading in every README is
/// deleted. There is no `LineComment` in this descriptor and there must never
/// be one. Markdown has exactly one comment form: the HTML one.
///
/// ## Fences
///
/// A fenced block is a `Region` with `Engine::Verbatim`. This is not a
/// nice-to-have: ``` ```html <!-- example --> ``` ``` is a *documented* snippet
/// in half the READMEs in the world, and lexing it as markup purges the example
/// out of the documentation. Nothing in SPEC §1/§4 can express "do not scan
/// this", which is REGION GAP 1 in `markup-data.md`.
///
/// The close is ` ``` ` wherever it appears; a fence containing a literal
/// ` ``` ` must itself be fenced with four or more backticks, so the "first
/// match" rule is correct for the fence length that opened the region. A
/// stricter engine may require the close to be at `At::LineStart` (see REGION
/// GAP 3) — that would also fix the four-backtick case.
///
/// ## Inline code
///
/// A single-backtick quote stops `` `<!-- -->` `` in prose from being purged.
/// `multiline: false` is deliberate: allowing it to run across lines lets one
/// stray backtick swallow the rest of the document. The cost of `false` is that
/// an unmatched backtick yields `Bail("unterminated string")` and the file is
/// `Skipped` — the safe direction (nothing purged) rather than the destructive
/// one.
pub static MARKDOWN: Syntax = Syntax {
    name: "markdown",
    extensions: &["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "ronn"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "prettier-ignore",
        "markdownlint-disable",
        "markdownlint-enable",
        "markdownlint-configure-file",
        "mdformat",
        "vale",
        "toc",
    ],
    regions: &[
        Region {
            open: "```",
            close: "```",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "~~~",
            close: "~~~",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// MDX
// ---------------------------------------------------------------------------

/// `.mdx`.
///
/// Markdown's rules plus MDX's two additions:
///
/// * `{/* … */}` — a JSX comment. Expressing it as a `BlockComment` with
///   `open: "{/*"` instead of a `/*` line in `line`/`block` is the whole trick:
///   `/*` outside braces is plain Markdown text and must not be touched.
/// * `<script>` / `<style>` — MDX passes arbitrary JSX through, but the two
///   tags whose bodies are genuinely another language are these.
///
/// The fence regions are inherited from `MARKDOWN` for the same reason, and the
/// `#`-is-a-heading rule applies here too.
pub static MDX: Syntax = Syntax {
    name: "mdx",
    extensions: &["mdx"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[
        BlockComment {
            open: "<!--",
            close: "-->",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{/*",
            close: "*/}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "prettier-ignore",
        "markdownlint-disable",
        "markdownlint-enable",
    ],
    regions: &[
        Region {
            open: "```",
            close: "```",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "~~~",
            close: "~~~",
            engine: Engine::Verbatim,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Vue
// ---------------------------------------------------------------------------

/// `.vue` — single-file components.
///
/// Three regions, `Markup` for `<template>` so nested `<template>` blocks work
/// (SPEC §4 explicitly allows nested regions).
///
/// # `//` lives in the script region, NOT in this descriptor
///
/// `Region.engine` selects an *engine*, so `Engine::Script` must bring the
/// canonical JavaScript descriptor with it (its `//`, `/* */`, regex and JSX
/// state). If instead the embedded engine re-read the *host* descriptor, Vue
/// would need `//` in `line` here — and then `<p>See http://example.com</p>`
/// turns the rest of the template into a comment. Same for `<style>` and CSS
/// `/* */`. This is REGION GAP 4 in `markup-data.md`.
///
/// # `lang="ts"` is not expressible
///
/// `needs_attr` is a presence test, so `Region.engine` cannot vary by attribute
/// **value**. For `lang="ts"`/`lang="js"` this is harmless (both use `//` and
/// `/* */`, so the purged spans are identical), but it is wrong for
/// `<script lang="coffee">` (`#`), `<style lang="stylus">` (indented comments)
/// and `<template lang="pug">` (`//-`). REGION GAP 2. Until `attr_value` exists
/// those blocks are lexed with the default engine — over-retention (comments
/// left in) where the dialect differs, never silent corruption.
///
/// Custom blocks (`<i18n>`, `<docs>`, `<route>`) are not modelled; their bodies
/// are lexed as markup, so `//` comments inside a `<i18n>` JSON block survive.
/// Under-purge, not corruption.
pub static VUE: Syntax = Syntax {
    name: "vue",
    extensions: &["vue"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "prettier-ignore",
        "vue-ignore",
        "cspell:disable",
        "cspell:enable",
    ],
    regions: &[
        Region {
            open: "<template",
            close: "</template>",
            engine: Engine::Markup,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Svelte
// ---------------------------------------------------------------------------

/// `.svelte`.
///
/// Svelte has **no `<template>` wrapper** — the markup is the file body, so
/// there are only two regions. Markup comments come in two flavours:
///
/// * `<!-- -->` — a *rendered* HTML comment. It reaches the browser, so purging
///   it changes the shipped DOM. That is exactly what doc-purge is for, but it
///   is the reason this one is worth reviewing in `--diff` output.
/// * `{/* … */}` — a Svelte/JSX expression comment, stripped at compile time and
///   never shipped. Purging it is a pure no-op for output.
///
/// `{#if}` / `{:else}` / `{/if}` / `{@html}` are *directives*, not comments —
/// do not add `{#` or `{@` to `block`.
pub static SVELTE: Syntax = Syntax {
    name: "svelte",
    extensions: &["svelte"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[
        BlockComment {
            open: "<!--",
            close: "-->",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{/*",
            close: "*/}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "prettier-ignore",
        "svelte-ignore",
        "cspell:disable",
        "cspell:enable",
    ],
    regions: &[
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// Astro
// ---------------------------------------------------------------------------

/// `.astro`.
///
/// Two languages in one file in an order that matters:
///
/// 1. A **leading frontmatter fence** — `---` on its own line at byte 0,
///    TypeScript body, `---` on its own line to close.
/// 2. An HTML body with JSX-ish `{/* */}` expressions and rendered
///    `<!-- -->` comments.
///
/// # The frontmatter is REGION GAP 3
///
/// `Region` has no anchor and no count, so the entry below cannot say "only at
/// the top of the file" nor "only once". Concretely it is wrong today in two
/// ways: a body-level `<hr>`-ish `---` (or a `---` in a Markdown-in-Astro
/// snippet) can open a "frontmatter" region mid-file, and the *closing* `---`
/// must be at line start but `Region.close` is a plain byte match.
///
/// Recommended shape (both fields are additions to `Region`):
///
/// ```ignore
/// at: At::StartOfFile,          // reuse At, extended with StartOfFile
/// close_at: At::LineStart,      // close only where only whitespace precedes
/// once: true,                   // consume at most one region
/// ```
///
/// `Engine::Verbatim` is *not* what the frontmatter needs — the body is real
/// TypeScript and must be lexed (`//`, `/* */`, template literals).
///
/// Note that `<script>` in `.astro` is TypeScript by default and is bundled
/// client-side; `<style>` is CSS/SCSS-by-lang.
pub static ASTRO: Syntax = Syntax {
    name: "astro",
    extensions: &["astro"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[
        BlockComment {
            open: "<!--",
            close: "-->",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
        BlockComment {
            open: "{/*",
            close: "*/}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        },
    ],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[
        "prettier-ignore",
        "astro-ignore",
        "cspell:disable",
        "cspell:enable",
    ],
    regions: &[
        // UNVERIFIED: needs `at: At::StartOfFile` + `once: true` (REGION GAP 3).
        // As written, any `---` line opens a frontmatter region.
        Region {
            open: "---",
            close: "---",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Script,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<style",
            close: "</style>",
            engine: Engine::Style,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// XSLT
// ---------------------------------------------------------------------------

/// `.xsl` / `.xslt`.
///
/// XSLT is XML: one comment form, XML's `--` rule, no regions.
///
/// # Two traps specific to XSLT
///
/// * **Do not add `<script>`/`<style>` regions.** In XSLT a `<script>` is a
///   *literal result element* — the text between the tags is a stylesheet
///   template (XML + `${}`/`<xsl:value-of>`), not JavaScript. Mapping it to
///   `Engine::Script` would purge `//` out of XPath-ish template text.
/// * **`<xsl:comment>` is output, not a comment.** `<xsl:comment>generated</xsl:comment>`
///   emits `<!-- generated -->` into the *result* document; the text inside is
///   load-bearing output. It is an element, not a `BlockComment`, so the
///   descriptor is already correct — this note exists so nobody "fixes" it.
///   Same for `<xsl:text>` and `<xsl:message>`.
///
/// `disable-output-escaping` bodies that contain a literal `<!--` are CDATA
/// wrapped; see the CDATA note on `XML`.
pub static XSLT: Syntax = Syntax {
    name: "xslt",
    extensions: &["xsl", "xslt"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[Region {
        open: "<![CDATA[",
        close: "]]>",
        engine: Engine::Verbatim,
        needs_attr: None,
        close_at: At::Anywhere,
        once: false,
    }],
};

// ---------------------------------------------------------------------------
// Property list
// ---------------------------------------------------------------------------

/// `.plist` — XML property lists.
///
/// XML comment rules, no regions.
///
/// Two detection/robustness notes:
///
/// * **Binary plists** (`bplist00` magic) are not text. `detect` should sniff
///   the magic and refuse the file; feeding a binary plist to a byte scanner
///   gives random matches and, worse, could rewrite bytes. `Bail` is the right
///   answer, so no descriptor change is needed — but the sniff must exist.
/// * `<data>` elements hold base64. The base64 alphabet has no `<` and no `-`,
///   so it can never contain `<!--` or `-->` and needs no `Verbatim` region.
///   That is luck, not design; if `<data>` ever becomes a `Verbatim` region it
///   costs nothing.
///
/// `<string>`/`<date>`/`<integer>` are plain text.
pub static PLIST: Syntax = Syntax {
    name: "plist",
    extensions: &["plist"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
    engine: Engine::Markup,
    line: &[],
    block: &[BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    }],
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &[],
    regions: &[Region {
        open: "<![CDATA[",
        close: "]]>",
        engine: Engine::Verbatim,
        needs_attr: None,
        close_at: At::Anywhere,
        once: false,
    }],
};

// ---------------------------------------------------------------------------
// QML
// ---------------------------------------------------------------------------

/// `.qml` — Qt Quick / Qt 6 QML.
///
/// **`Engine::Script`, not `Engine::Markup`.** QML looks like a markup language
/// (`Item { width: 100 }`) but every `{}` body is JavaScript, and the comment
/// syntax is JavaScript's: `//` and `/* */` with no HTML comments anywhere. The
/// `Script` engine is the one that already knows about regex-vs-division and
/// template literals, both of which appear in real QML bindings
/// (`` text: `/a/g.test(x) ? `n=${y}` : ""` ``).
///
/// `Family::Curly` so `purge.rs`'s `merging()` treats adjacent `//` runs the way
/// it does for JS — QML property blocks are commonly commented out line by line.
///
/// No regions: `<script>`/`<style>` do not exist here, and QML has no embedded
/// other-language blocks (a `Component` is still QML).
///
/// UNVERIFIED: `Engine::Script` must not require a JSX-capable dialect to lex
/// this — `markup()` in js.rs:106 gates JSX on the `Dialect`, and QML needs it
/// gated off. With the 2.0 signature change it must key off the descriptor, not
/// the enum.
pub static QML: Syntax = Syntax {
    name: "qml",
    extensions: &["qml"],
    filenames: &[],
    shebangs: &[],
    family: Family::Curly,
    engine: Engine::Script,
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
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["qml:"],
    regions: &[],
};

// ---------------------------------------------------------------------------
// HEEx
// ---------------------------------------------------------------------------

/// `.heex` / `.leex` — Phoenix LiveView HTML-aware EEx.
///
/// Region stack: HTML in the host, Elixir inside `<% … %>` and `<%= … %>`.
///
/// ```text
/// <%# this is a comment %>          <- purged (BlockComment "<%#")
/// <%= render_slot(@inner) %>        <- Elixir region
/// <p class={@class}>text</p>        <- HTML outside regions
/// ```
///
/// # Elixir inside the regions
///
/// There is no `Engine::Elixir`. Elixir is `Family::Hash` with `#` comments,
/// heredocs and `@moduledoc`-style `DocStyle::Attribute`, which is exactly what
/// `Engine::Indented` generalised over `DocStyle` is for (SPEC §1 lists
/// `DocStyle::Attribute { attributes: ["@moduledoc", "@doc", "@typedoc"] }` but
/// §4 only assigns `DocStyle::Body` to `Indented`). Using `Engine::Indented`
/// here assumes that generalisation lands — **UNVERIFIED**.
///
/// # `%>` inside an Elixir string
///
/// `<%= "100%> done" %>` is legal. `Region.close` detection must be delegated to
/// the region's engine (which knows Elixir strings), not done by a raw byte
/// scan for `%>`. That requirement is REGION GAP 4(b) in `markup-data.md`.
///
/// # Rendered vs silent — the trap for this dialect
///
/// * `<%# … %>` is a **silent** EEx comment: it emits nothing. Purging it is
///   output-neutral.
/// * `<!-- … -->` in a HEEx template is a **rendered** HTML comment: it is sent
///   to the browser. Purging it changes the shipped page. It is still a comment
///   (that is the tool's job), but it belongs in the "output changes" bucket,
///   not the "dead syntax" bucket.
/// * `<%!-- … --%>` is the HEEx-specific silent block comment and is preferred
///   over `<!-- -->` precisely because it does not render. It is listed below,
///   marked UNVERIFIED.
///
/// UNVERIFIED: whether Phoenix's HEEx engine still accepts `<!-- -->` at all
/// (recent versions raise on unescaped HTML comments in some contexts). Both
/// forms are declared; when in doubt the silent one is the one to purge.
///
/// Note `<%%` is the escape for a literal `<%` and must **not** open a region —
/// add it to the region-open rejection set.
pub static HEEX: Syntax = Syntax {
    name: "heex",
    extensions: &["heex", "leex"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
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
        // UNVERIFIED: exact spelling/availability of the silent HEEx comment.
        BlockComment {
            open: "<%!--",
            close: "--%>",
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
    directives: &["phx-", "cspell:disable", "cspell:enable"],
    regions: &[
        Region {
            open: "<%=",
            close: "%>",
            engine: Engine::Indented,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<%",
            close: "%>",
            engine: Engine::Indented,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

// ---------------------------------------------------------------------------
// EEX
// ---------------------------------------------------------------------------

/// `.eex` — plain EEx (and `.html.eex`).
///
/// Identical comment rules to `HEEX` minus the HEEx-specific `<!-- -->`
/// distinction: in `.eex` the HTML is literal output and `<!-- -->` is a plain
/// HTML comment.
///
/// `<%# … %>` (silent) is a `BlockComment`; `<%=` / `<%` are Elixir regions.
/// `<%#` must be matched **before** `<%` — longest open first.
///
/// EEx has no `{% %}` and no `{{ }}`; those belong to `TEMPLATE` (Jinja et al.).
pub static EEX: Syntax = Syntax {
    name: "eex",
    extensions: &["eex"],
    filenames: &[],
    shebangs: &[],
    family: Family::Markup,
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
    directives: &["phx-", "cspell:disable", "cspell:enable"],
    regions: &[
        Region {
            open: "<%=",
            close: "%>",
            engine: Engine::Indented,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
        Region {
            open: "<%",
            close: "%>",
            engine: Engine::Indented,
            needs_attr: None,

            close_at: At::Anywhere,
            once: false,
        },
    ],
};

/// Registry table for `Family::Markup`. `QML` is included because it belongs to
/// this wave's file even though its `family` is `Curly`.
pub static MARKUP: &[(&str, &Syntax)] = &[
    ("html", &HTML),
    ("xhtml", &XHTML),
    ("xml", &XML),
    ("svg", &SVG),
    ("markdown", &MARKDOWN),
    ("mdx", &MDX),
    ("vue", &VUE),
    ("svelte", &SVELTE),
    ("astro", &ASTRO),
    ("xslt", &XSLT),
    ("plist", &PLIST),
    ("qml", &QML),
    ("heex", &HEEX),
    ("eex", &EEX),
];
