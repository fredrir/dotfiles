
use crate::syntax::*;

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
