
use crate::syntax::*;

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
    quotes: &[],
    heredocs: &[],
    doc: DocStyle::None,
    directives: &["prettier-ignore"],
    regions: &[],
};

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
    directives: &["!", "prettier-ignore"],
    regions: &[],
};

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
