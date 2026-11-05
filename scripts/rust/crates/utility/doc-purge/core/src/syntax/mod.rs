
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Generic,
    Curly,
    Dash,
    Script,
    Style,
    Indented,
    Shell,
    Ruby,
    Perl,
    Markup,
    Template,
    Verbatim,
    Elixir,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum At {
    Anywhere,
    LineStart,
    WordStart,
    StartOfFile,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineComment {
    pub marker: &'static str,
    pub at: At,
    pub continues_on_backslash: bool,
    pub doc: bool,
    pub owns_indent_block: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockComment {
    pub open: &'static str,
    pub close: &'static str,
    pub nested: bool,
    pub at: At,
    pub important: Option<char>,
    pub to_eof_ok: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Escape {
    Backslash,
    None,
    Doubling,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quote {
    pub open: &'static str,
    pub close: &'static str,
    pub escape: Escape,
    pub multiline: bool,
    pub prefixes: &'static [&'static str],
    pub hashes: bool,
    pub raw_tag: bool,
    pub text: Option<TextGlyphs>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextGlyphs {
    pub escapes: bool,
    pub folds: bool,
    pub delim: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Heredoc {
    pub openers: &'static [&'static str],
    pub strip_tabs: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocStyle {
    None,
    Body {
        openers: &'static [&'static str],
        delim: &'static str,
    },
    Attribute {
        attributes: &'static [&'static str],
    },
    Before {
        keywords: &'static [&'static str],
    },
    AfterName,
    Ldoc,
    CommentOnly,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Region {
    pub open: &'static str,
    pub close: &'static str,
    pub engine: Engine,
    pub needs_attr: Option<(&'static str, &'static str)>,
    pub close_at: At,
    pub once: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Syntax {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    pub shebangs: &'static [&'static str],
    pub family: Family,
    pub engine: Engine,
    pub line: &'static [LineComment],
    pub block: &'static [BlockComment],
    pub quotes: &'static [Quote],
    pub heredocs: &'static [Heredoc],
    pub doc: DocStyle,
    pub directives: &'static [&'static str],
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
