//! The languages doc-purge reads.
//!
//! Generated from `syntax/*.rs`: `Dialect`, `Dialect::ALL`, `LANGUAGES` and
//! `Dialect::syntax()` all come from one table, so they cannot disagree.

use std::path::Path;

pub use crate::syntax::{Family, Syntax};

use crate::syntax::{config, curly, dash, hash, markup, script, shell_ruby, style, template};

/// One language doc-purge can read.
///
/// A closed set: every variant has a descriptor, and `syntax()` is an
/// exhaustive match, so a new variant does not compile until it has one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    Rust,
    Go,
    C,
    Cpp,
    Java,
    CSharp,
    Kotlin,
    Swift,
    Zig,
    Dart,
    Scala,
    Groovy,
    Solidity,
    ObjectiveC,
    Vala,
    D,
    Haxe,
    Verilog,
    Wgsl,
    Glsl,
    Gleam,
    Odin,
    V,
    Roc,
    JavaScript,
    Mjs,
    TypeScript,
    Jsx,
    Tsx,
    Qml,
    Python,
    Toml,
    Yaml,
    Jsonc,
    Json5,
    Ini,
    Shell,
    Ruby,
    Elixir,
    Crystal,
    Nim,
    Perl,
    R,
    Julia,
    PowerShell,
    Tcl,
    Fish,
    Awk,
    Gdscript,
    Dockerfile,
    Makefile,
    Cmake,
    Justfile,
    Starlark,
    Hcl,
    Lua,
    Sql,
    Haskell,
    Ada,
    Vhdl,
    Fortran,
    FortranFixed,
    Matlab,
    Elm,
    PureScript,
    Ocaml,
    FSharp,
    Sml,
    Clojure,
    Scheme,
    Racket,
    CommonLisp,
    EmacsLisp,
    Assembly,
    Viml,
    Css,
    Scss,
    Sass,
    Less,
    Stylus,
    Sugarss,
    Html,
    Xhtml,
    Xml,
    Svg,
    Xslt,
    Plist,
    Markdown,
    Mdx,
    Vue,
    Svelte,
    Astro,
    Heex,
    Eex,
    Jinja,
    Twig,
    Handlebars,
    Mustache,
    Liquid,
    Erb,
    Ejs,
    Pug,
    Haml,
    Slim,
}

impl Dialect {
    /// Every dialect, in registry order.
    pub const ALL: &'static [Dialect] = &[
        Dialect::Rust,
        Dialect::Go,
        Dialect::C,
        Dialect::Cpp,
        Dialect::Java,
        Dialect::CSharp,
        Dialect::Kotlin,
        Dialect::Swift,
        Dialect::Zig,
        Dialect::Dart,
        Dialect::Scala,
        Dialect::Groovy,
        Dialect::Solidity,
        Dialect::ObjectiveC,
        Dialect::Vala,
        Dialect::D,
        Dialect::Haxe,
        Dialect::Verilog,
        Dialect::Wgsl,
        Dialect::Glsl,
        Dialect::Gleam,
        Dialect::Odin,
        Dialect::V,
        Dialect::Roc,
        Dialect::JavaScript,
        Dialect::Mjs,
        Dialect::TypeScript,
        Dialect::Jsx,
        Dialect::Tsx,
        Dialect::Qml,
        Dialect::Python,
        Dialect::Toml,
        Dialect::Yaml,
        Dialect::Jsonc,
        Dialect::Json5,
        Dialect::Ini,
        Dialect::Shell,
        Dialect::Ruby,
        Dialect::Elixir,
        Dialect::Crystal,
        Dialect::Nim,
        Dialect::Perl,
        Dialect::R,
        Dialect::Julia,
        Dialect::PowerShell,
        Dialect::Tcl,
        Dialect::Fish,
        Dialect::Awk,
        Dialect::Gdscript,
        Dialect::Dockerfile,
        Dialect::Makefile,
        Dialect::Cmake,
        Dialect::Justfile,
        Dialect::Starlark,
        Dialect::Hcl,
        Dialect::Lua,
        Dialect::Sql,
        Dialect::Haskell,
        Dialect::Ada,
        Dialect::Vhdl,
        Dialect::Fortran,
        Dialect::FortranFixed,
        Dialect::Matlab,
        Dialect::Elm,
        Dialect::PureScript,
        Dialect::Ocaml,
        Dialect::FSharp,
        Dialect::Sml,
        Dialect::Clojure,
        Dialect::Scheme,
        Dialect::Racket,
        Dialect::CommonLisp,
        Dialect::EmacsLisp,
        Dialect::Assembly,
        Dialect::Viml,
        Dialect::Css,
        Dialect::Scss,
        Dialect::Sass,
        Dialect::Less,
        Dialect::Stylus,
        Dialect::Sugarss,
        Dialect::Html,
        Dialect::Xhtml,
        Dialect::Xml,
        Dialect::Svg,
        Dialect::Xslt,
        Dialect::Plist,
        Dialect::Markdown,
        Dialect::Mdx,
        Dialect::Vue,
        Dialect::Svelte,
        Dialect::Astro,
        Dialect::Heex,
        Dialect::Eex,
        Dialect::Jinja,
        Dialect::Twig,
        Dialect::Handlebars,
        Dialect::Mustache,
        Dialect::Liquid,
        Dialect::Erb,
        Dialect::Ejs,
        Dialect::Pug,
        Dialect::Haml,
        Dialect::Slim,
    ];

    /// The descriptor behind this dialect.
    pub fn syntax(self) -> &'static Syntax {
        match self {
            Dialect::Rust => &curly::RUST,
            Dialect::Go => &curly::GO,
            Dialect::C => &curly::C,
            Dialect::Cpp => &curly::CPP,
            Dialect::Java => &curly::JAVA,
            Dialect::CSharp => &curly::CSHARP,
            Dialect::Kotlin => &curly::KOTLIN,
            Dialect::Swift => &curly::SWIFT,
            Dialect::Zig => &curly::ZIG,
            Dialect::Dart => &curly::DART,
            Dialect::Scala => &curly::SCALA,
            Dialect::Groovy => &curly::GROOVY,
            Dialect::Solidity => &curly::SOLIDITY,
            Dialect::ObjectiveC => &curly::OBJECTIVE_C,
            Dialect::Vala => &curly::VALA,
            Dialect::D => &curly::D,
            Dialect::Haxe => &curly::HAXE,
            Dialect::Verilog => &curly::VERILOG,
            Dialect::Wgsl => &curly::WGSL,
            Dialect::Glsl => &curly::GLSL,
            Dialect::Gleam => &curly::GLEAM,
            Dialect::Odin => &curly::ODIN,
            Dialect::V => &curly::V,
            Dialect::Roc => &curly::ROC,
            Dialect::JavaScript => &script::JAVASCRIPT,
            Dialect::Mjs => &script::MJS,
            Dialect::TypeScript => &script::TYPESCRIPT,
            Dialect::Jsx => &script::JSX,
            Dialect::Tsx => &script::TSX,
            Dialect::Qml => &markup::QML,
            Dialect::Python => &hash::PYTHON,
            Dialect::Toml => &hash::TOML,
            Dialect::Yaml => &hash::YAML,
            Dialect::Jsonc => &config::JSONC,
            Dialect::Json5 => &config::JSON5,
            Dialect::Ini => &hash::INI,
            Dialect::Shell => &shell_ruby::SHELL,
            Dialect::Ruby => &shell_ruby::RUBY,
            Dialect::Elixir => &hash::ELIXIR,
            Dialect::Crystal => &hash::CRYSTAL,
            Dialect::Nim => &hash::NIM,
            Dialect::Perl => &hash::PERL,
            Dialect::R => &hash::R,
            Dialect::Julia => &hash::JULIA,
            Dialect::PowerShell => &hash::POWERSHELL,
            Dialect::Tcl => &hash::TCL,
            Dialect::Fish => &hash::FISH,
            Dialect::Awk => &hash::AWK,
            Dialect::Gdscript => &hash::GDSCRIPT,
            Dialect::Dockerfile => &hash::DOCKERFILE,
            Dialect::Makefile => &hash::MAKEFILE,
            Dialect::Cmake => &hash::CMAKE,
            Dialect::Justfile => &hash::JUSTFILE,
            Dialect::Starlark => &hash::STARLARK,
            Dialect::Hcl => &hash::HCL,
            Dialect::Lua => &dash::LUA,
            Dialect::Sql => &dash::SQL,
            Dialect::Haskell => &dash::HASKELL,
            Dialect::Ada => &dash::ADA,
            Dialect::Vhdl => &dash::VHDL,
            Dialect::Fortran => &dash::FORTRAN,
            Dialect::FortranFixed => &dash::FORTRAN_FIXED,
            Dialect::Matlab => &dash::MATLAB,
            Dialect::Elm => &dash::ELM,
            Dialect::PureScript => &dash::PURE_SCRIPT,
            Dialect::Ocaml => &dash::OCAML,
            Dialect::FSharp => &dash::FSHARP,
            Dialect::Sml => &dash::SML,
            Dialect::Clojure => &dash::CLOJURE,
            Dialect::Scheme => &dash::SCHEME,
            Dialect::Racket => &dash::RACKET,
            Dialect::CommonLisp => &dash::COMMON_LISP,
            Dialect::EmacsLisp => &dash::EMACS_LISP,
            Dialect::Assembly => &dash::ASSEMBLY,
            Dialect::Viml => &dash::VIML,
            Dialect::Css => &style::CSS,
            Dialect::Scss => &style::SCSS,
            Dialect::Sass => &style::SASS,
            Dialect::Less => &style::LESS,
            Dialect::Stylus => &style::STYLUS,
            Dialect::Sugarss => &style::SUGARSS,
            Dialect::Html => &markup::HTML,
            Dialect::Xhtml => &markup::XHTML,
            Dialect::Xml => &markup::XML,
            Dialect::Svg => &markup::SVG,
            Dialect::Xslt => &markup::XSLT,
            Dialect::Plist => &markup::PLIST,
            Dialect::Markdown => &markup::MARKDOWN,
            Dialect::Mdx => &markup::MDX,
            Dialect::Vue => &markup::VUE,
            Dialect::Svelte => &markup::SVELTE,
            Dialect::Astro => &markup::ASTRO,
            Dialect::Heex => &markup::HEEX,
            Dialect::Eex => &markup::EEX,
            Dialect::Jinja => &template::JINJA,
            Dialect::Twig => &template::TWIG,
            Dialect::Handlebars => &template::HANDLEBARS,
            Dialect::Mustache => &template::MUSTACHE,
            Dialect::Liquid => &template::LIQUID,
            Dialect::Erb => &template::ERB,
            Dialect::Ejs => &template::EJS,
            Dialect::Pug => &template::PUG,
            Dialect::Haml => &template::HAML,
            Dialect::Slim => &template::SLIM,
        }
    }

    /// How this language groups with the languages it is lexed like.
    pub fn family(self) -> crate::syntax::Family {
        self.syntax().family
    }

    /// Which engine lexes this language.
    pub fn engine(self) -> crate::syntax::Engine {
        self.syntax().engine
    }
}

/// A language and the extensions it claims.
pub struct Language {
    pub name: &'static str,
    pub dialect: Dialect,
    pub extensions: &'static [&'static str],
}

/// Every language, in the order `for_path` prefers.
pub const LANGUAGES: &[Language] = &[
    Language {
        name: "rust",
        dialect: Dialect::Rust,
        extensions: &["rs"],
    },
    Language {
        name: "go",
        dialect: Dialect::Go,
        extensions: &["go"],
    },
    Language {
        name: "c",
        dialect: Dialect::C,
        extensions: &["c", "h"],
    },
    Language {
        name: "cpp",
        dialect: Dialect::Cpp,
        extensions: &["cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "ipp", "inl", "ixx", "tcc", "tpp"],
    },
    Language {
        name: "java",
        dialect: Dialect::Java,
        extensions: &["java"],
    },
    Language {
        name: "csharp",
        dialect: Dialect::CSharp,
        extensions: &["cs", "csx"],
    },
    Language {
        name: "kotlin",
        dialect: Dialect::Kotlin,
        extensions: &["kt", "kts"],
    },
    Language {
        name: "swift",
        dialect: Dialect::Swift,
        extensions: &["swift"],
    },
    Language {
        name: "zig",
        dialect: Dialect::Zig,
        extensions: &["zig"],
    },
    Language {
        name: "dart",
        dialect: Dialect::Dart,
        extensions: &["dart"],
    },
    Language {
        name: "scala",
        dialect: Dialect::Scala,
        extensions: &["scala", "sc", "sbt"],
    },
    Language {
        name: "groovy",
        dialect: Dialect::Groovy,
        extensions: &["groovy", "gvy", "gy", "gsh"],
    },
    Language {
        name: "solidity",
        dialect: Dialect::Solidity,
        extensions: &["sol"],
    },
    Language {
        name: "objective-c",
        dialect: Dialect::ObjectiveC,
        extensions: &["m", "mm", "h"],
    },
    Language {
        name: "vala",
        dialect: Dialect::Vala,
        extensions: &["vala", "vapi"],
    },
    Language {
        name: "d",
        dialect: Dialect::D,
        extensions: &["d", "di"],
    },
    Language {
        name: "haxe",
        dialect: Dialect::Haxe,
        extensions: &["hx"],
    },
    Language {
        name: "verilog",
        dialect: Dialect::Verilog,
        extensions: &["v", "sv", "vh", "svh"],
    },
    Language {
        name: "wgsl",
        dialect: Dialect::Wgsl,
        extensions: &["wgsl"],
    },
    Language {
        name: "glsl",
        dialect: Dialect::Glsl,
        extensions: &["glsl", "vert", "frag", "geom", "comp", "tesc", "tese", "rgen", "rint", "rmiss", "rchit", "rahit", "vsh", "fsh", "gsh", "vs", "fs"],
    },
    Language {
        name: "gleam",
        dialect: Dialect::Gleam,
        extensions: &["gleam"],
    },
    Language {
        name: "odin",
        dialect: Dialect::Odin,
        extensions: &["odin"],
    },
    Language {
        name: "v",
        dialect: Dialect::V,
        extensions: &["v", "vsh"],
    },
    Language {
        name: "roc",
        dialect: Dialect::Roc,
        extensions: &["roc"],
    },
    Language {
        name: "javascript",
        dialect: Dialect::JavaScript,
        extensions: &["js", "cjs"],
    },
    Language {
        name: "mjs",
        dialect: Dialect::Mjs,
        extensions: &["mjs"],
    },
    Language {
        name: "typescript",
        dialect: Dialect::TypeScript,
        extensions: &["ts", "mts", "cts"],
    },
    Language {
        name: "jsx",
        dialect: Dialect::Jsx,
        extensions: &["jsx"],
    },
    Language {
        name: "tsx",
        dialect: Dialect::Tsx,
        extensions: &["tsx"],
    },
    Language {
        name: "qml",
        dialect: Dialect::Qml,
        extensions: &["qml"],
    },
    Language {
        name: "python",
        dialect: Dialect::Python,
        extensions: &["py", "pyi", "pyw", "pyx", "pxd"],
    },
    Language {
        name: "toml",
        dialect: Dialect::Toml,
        extensions: &["toml"],
    },
    Language {
        name: "yaml",
        dialect: Dialect::Yaml,
        extensions: &["yaml", "yml"],
    },
    Language {
        name: "jsonc",
        dialect: Dialect::Jsonc,
        extensions: &["jsonc"],
    },
    Language {
        name: "json5",
        dialect: Dialect::Json5,
        extensions: &["json5"],
    },
    Language {
        name: "ini",
        dialect: Dialect::Ini,
        extensions: &["ini", "cfg", "conf"],
    },
    Language {
        name: "shell",
        dialect: Dialect::Shell,
        extensions: &["sh", "bash", "zsh", "ksh"],
    },
    Language {
        name: "ruby",
        dialect: Dialect::Ruby,
        extensions: &["rb", "rake", "gemspec", "ru", "podspec", "jbuilder", "thor"],
    },
    Language {
        name: "elixir",
        dialect: Dialect::Elixir,
        extensions: &["ex", "exs"],
    },
    Language {
        name: "crystal",
        dialect: Dialect::Crystal,
        extensions: &["cr"],
    },
    Language {
        name: "nim",
        dialect: Dialect::Nim,
        extensions: &["nim", "nims", "nimble"],
    },
    Language {
        name: "perl",
        dialect: Dialect::Perl,
        extensions: &["pl", "pm", "t", "psgi"],
    },
    Language {
        name: "r",
        dialect: Dialect::R,
        extensions: &["r", "R"],
    },
    Language {
        name: "julia",
        dialect: Dialect::Julia,
        extensions: &["jl"],
    },
    Language {
        name: "powershell",
        dialect: Dialect::PowerShell,
        extensions: &["ps1", "psm1", "psd1"],
    },
    Language {
        name: "tcl",
        dialect: Dialect::Tcl,
        extensions: &["tcl", "tk"],
    },
    Language {
        name: "fish",
        dialect: Dialect::Fish,
        extensions: &["fish"],
    },
    Language {
        name: "awk",
        dialect: Dialect::Awk,
        extensions: &["awk"],
    },
    Language {
        name: "gdscript",
        dialect: Dialect::Gdscript,
        extensions: &["gd"],
    },
    Language {
        name: "dockerfile",
        dialect: Dialect::Dockerfile,
        extensions: &["dockerfile"],
    },
    Language {
        name: "makefile",
        dialect: Dialect::Makefile,
        extensions: &["mk", "make"],
    },
    Language {
        name: "cmake",
        dialect: Dialect::Cmake,
        extensions: &["cmake"],
    },
    Language {
        name: "justfile",
        dialect: Dialect::Justfile,
        extensions: &[],
    },
    Language {
        name: "starlark",
        dialect: Dialect::Starlark,
        extensions: &["bzl", "bazel"],
    },
    Language {
        name: "hcl",
        dialect: Dialect::Hcl,
        extensions: &["tf", "tfvars", "hcl"],
    },
    Language {
        name: "lua",
        dialect: Dialect::Lua,
        extensions: &["lua"],
    },
    Language {
        name: "sql",
        dialect: Dialect::Sql,
        extensions: &["sql", "ddl", "dml"],
    },
    Language {
        name: "haskell",
        dialect: Dialect::Haskell,
        extensions: &["hs"],
    },
    Language {
        name: "ada",
        dialect: Dialect::Ada,
        extensions: &["adb", "ads", "ada", "ad", "gpr"],
    },
    Language {
        name: "vhdl",
        dialect: Dialect::Vhdl,
        extensions: &["vhd", "vhdl"],
    },
    Language {
        name: "fortran",
        dialect: Dialect::Fortran,
        extensions: &["f90", "f95", "f03", "f08", "f18"],
    },
    Language {
        name: "fortran-fixed",
        dialect: Dialect::FortranFixed,
        extensions: &["f", "for", "f77", "ftn"],
    },
    Language {
        name: "matlab",
        dialect: Dialect::Matlab,
        extensions: &["m"],
    },
    Language {
        name: "elm",
        dialect: Dialect::Elm,
        extensions: &["elm"],
    },
    Language {
        name: "purescript",
        dialect: Dialect::PureScript,
        extensions: &["purs"],
    },
    Language {
        name: "ocaml",
        dialect: Dialect::Ocaml,
        extensions: &["ml", "mli"],
    },
    Language {
        name: "fsharp",
        dialect: Dialect::FSharp,
        extensions: &["fs", "fsi", "fsx", "fsscript"],
    },
    Language {
        name: "sml",
        dialect: Dialect::Sml,
        extensions: &["sml", "sig", "fun"],
    },
    Language {
        name: "clojure",
        dialect: Dialect::Clojure,
        extensions: &["clj", "cljs", "cljc", "edn"],
    },
    Language {
        name: "scheme",
        dialect: Dialect::Scheme,
        extensions: &["scm", "ss", "sls", "sps", "sld"],
    },
    Language {
        name: "racket",
        dialect: Dialect::Racket,
        extensions: &["rkt", "rktd", "rktl", "scrbl"],
    },
    Language {
        name: "common-lisp",
        dialect: Dialect::CommonLisp,
        extensions: &["lisp", "lsp", "cl", "asd", "ros"],
    },
    Language {
        name: "emacs-lisp",
        dialect: Dialect::EmacsLisp,
        extensions: &["el"],
    },
    Language {
        name: "assembly",
        dialect: Dialect::Assembly,
        extensions: &["s", "S", "asm", "nasm"],
    },
    Language {
        name: "viml",
        dialect: Dialect::Viml,
        extensions: &["vim", "vimrc"],
    },
    Language {
        name: "css",
        dialect: Dialect::Css,
        extensions: &["css", "pcss", "postcss"],
    },
    Language {
        name: "scss",
        dialect: Dialect::Scss,
        extensions: &["scss"],
    },
    Language {
        name: "sass",
        dialect: Dialect::Sass,
        extensions: &["sass"],
    },
    Language {
        name: "less",
        dialect: Dialect::Less,
        extensions: &["less"],
    },
    Language {
        name: "stylus",
        dialect: Dialect::Stylus,
        extensions: &["styl"],
    },
    Language {
        name: "sugarss",
        dialect: Dialect::Sugarss,
        extensions: &["sss"],
    },
    Language {
        name: "html",
        dialect: Dialect::Html,
        extensions: &["html", "htm"],
    },
    Language {
        name: "xhtml",
        dialect: Dialect::Xhtml,
        extensions: &["xhtml", "xht"],
    },
    Language {
        name: "xml",
        dialect: Dialect::Xml,
        extensions: &["xml", "xsd", "rng", "wsdl", "xul", "fodt", "fods", "xaml", "resx"],
    },
    Language {
        name: "svg",
        dialect: Dialect::Svg,
        extensions: &["svg"],
    },
    Language {
        name: "xslt",
        dialect: Dialect::Xslt,
        extensions: &["xsl", "xslt"],
    },
    Language {
        name: "plist",
        dialect: Dialect::Plist,
        extensions: &["plist"],
    },
    Language {
        name: "markdown",
        dialect: Dialect::Markdown,
        extensions: &["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "ronn"],
    },
    Language {
        name: "mdx",
        dialect: Dialect::Mdx,
        extensions: &["mdx"],
    },
    Language {
        name: "vue",
        dialect: Dialect::Vue,
        extensions: &["vue"],
    },
    Language {
        name: "svelte",
        dialect: Dialect::Svelte,
        extensions: &["svelte"],
    },
    Language {
        name: "astro",
        dialect: Dialect::Astro,
        extensions: &["astro"],
    },
    Language {
        name: "heex",
        dialect: Dialect::Heex,
        extensions: &["heex", "leex"],
    },
    Language {
        name: "eex",
        dialect: Dialect::Eex,
        extensions: &["eex"],
    },
    Language {
        name: "jinja",
        dialect: Dialect::Jinja,
        extensions: &["jinja", "jinja2", "j2"],
    },
    Language {
        name: "twig",
        dialect: Dialect::Twig,
        extensions: &["twig"],
    },
    Language {
        name: "handlebars",
        dialect: Dialect::Handlebars,
        extensions: &["hbs", "handlebars"],
    },
    Language {
        name: "mustache",
        dialect: Dialect::Mustache,
        extensions: &["mustache"],
    },
    Language {
        name: "liquid",
        dialect: Dialect::Liquid,
        extensions: &["liquid"],
    },
    Language {
        name: "erb",
        dialect: Dialect::Erb,
        extensions: &["erb", "rhtml"],
    },
    Language {
        name: "ejs",
        dialect: Dialect::Ejs,
        extensions: &["ejs"],
    },
    Language {
        name: "pug",
        dialect: Dialect::Pug,
        extensions: &["pug", "jade"],
    },
    Language {
        name: "haml",
        dialect: Dialect::Haml,
        extensions: &["haml"],
    },
    Language {
        name: "slim",
        dialect: Dialect::Slim,
        extensions: &["slim"],
    },
];

/// The extension-only fast path.
pub fn for_path(path: &Path) -> Option<&'static Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|language| language.extensions.contains(&extension.as_str()))
}

/// Extensionless files, matched on the file name.
pub fn for_name(name: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|language| {
        language
            .dialect
            .syntax()
            .filenames
            .iter()
            .any(|known| known.eq_ignore_ascii_case(name))
    })
}

/// The interpreter named by a `#!` line, if any.
pub fn for_shebang(line: &[u8]) -> Option<&'static Language> {
    let text = std::str::from_utf8(line).ok()?;
    let rest = text.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.next()?;
        if program.starts_with('-') {
            program = words.next()?;
        }
    }
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    LANGUAGES.iter().find(|language| {
        language
            .dialect
            .syntax()
            .shebangs
            .contains(&program)
    })
}

/// Find the language for a path, consulting the first line for a shebang and
/// sniffing the few extensions several languages claim.
pub fn detect(path: &Path, head: &[u8]) -> Option<&'static Language> {
    let name = path.file_name()?.to_str()?;
    if let Some(language) = for_name(name) {
        return Some(language);
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let claimed: Vec<&'static Language> = LANGUAGES
        .iter()
        .filter(|language| language.extensions.contains(&extension.as_str()))
        .collect();
    match claimed.len() {
        0 => return for_shebang(first_line(head)),
        1 => return claimed.first().copied(),
        _ => {}
    }
    if let Some(dialect) = sniff(&extension, head) {
        return LANGUAGES.iter().find(|language| language.dialect == dialect);
    }
    claimed.first().copied()
}

/// The languages that share an extension, disambiguated by what the file says.
///
/// Only comments and tokens that no other claimant uses are evidence; anything
/// ambiguous falls through to the first claimant in `LANGUAGES` order.
fn sniff(extension: &str, head: &[u8]) -> Option<Dialect> {
    let text = std::str::from_utf8(head).ok()?;
    let lines: Vec<&str> = text.lines().take(200).collect();
    let has = |needle: &str| lines.iter().any(|line| line.contains(needle));
    let trimmed = lines.iter().map(|line| line.trim_start()).collect::<Vec<_>>();
    let uses = |needle: &str| trimmed.iter().any(|line| line.starts_with(needle));
    match extension {
        // C, C++ and Objective-C all claim .h.
        "h" => {
            if has("@interface") || has("@implementation") || has("#import") {
                Some(Dialect::ObjectiveC)
            } else if has("template<") || has("template <") || has("namespace ") || has("class ")
                || has("std::") || has("public:") || has("private:")
            {
                Some(Dialect::Cpp)
            } else {
                None
            }
        }
        // Objective-C and MATLAB both claim .m.
        "m" => {
            if has("@interface") || has("@implementation") || has("#import") {
                Some(Dialect::ObjectiveC)
            } else if uses("%") || uses("function ") || has("end\n") {
                Some(Dialect::Matlab)
            } else {
                None
            }
        }
        // Verilog, V and Coq all claim .v.
        "v" => {
            if has("module ") && has("endmodule") || has("always @") || has("`timescale") {
                Some(Dialect::Verilog)
            } else if has("fn main()") || has("import os") || has("pub fn ") {
                Some(Dialect::V)
            } else {
                None
            }
        }
        // F# and GLSL both claim .fs.
        "fs" => {
            if has("void main") || has("#version") || has("gl_FragColor") {
                Some(Dialect::Glsl)
            } else {
                None
            }
        }
        // Perl and Prolog both claim .pl.
        "pl" => {
            if has(":- ") || has("?- ") {
                Some(Dialect::Perl)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn first_line(head: &[u8]) -> &[u8] {
    let end = head
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r'))
        .unwrap_or(head.len());
    &head[..end]
}

const ALIASES: &[(&str, &str)] = &[
    ("css-module", "css"),
    ("css-modules", "css"),
    ("cssmodules", "css"),
    ("module", "css"),
    ("tailwind", "css"),
    ("tailwindcss", "css"),
    ("node", "javascript"),
    ("nodejs", "javascript"),
    ("ecmascript", "javascript"),
    ("es", "javascript"),
    ("sass-indented", "sass"),
    ("indented", "sass"),
    ("sugar", "sugarss"),
    ("ts", "typescript"),
    ("js", "javascript"),
    ("py", "python"),
    ("rs", "rust"),
    ("sh", "shell"),
    ("bash", "shell"),
    ("zsh", "shell"),
    ("yml", "yaml"),
    ("md", "markdown"),
    ("hs", "haskell"),
    ("rb", "ruby"),
    ("ps1", "powershell"),
    ("tf", "hcl"),
];

/// Resolve a `--type` token: an alias, a language name or an extension.
pub fn for_token(token: &str) -> Option<&'static Language> {
    let wanted = token.trim().trim_start_matches('.').to_ascii_lowercase();
    if wanted.is_empty() {
        return None;
    }
    let wanted = ALIASES
        .iter()
        .find_map(|(alias, name)| (*alias == wanted.as_str()).then_some(*name))
        .unwrap_or(wanted.as_str());
    if let Some(language) = LANGUAGES
        .iter()
        .find(|language| language.name == wanted || language.extensions.contains(&wanted))
    {
        return Some(language);
    }
    let extension = wanted.rsplit('.').next()?;
    if extension == wanted {
        return None;
    }
    LANGUAGES
        .iter()
        .find(|language| language.extensions.contains(&extension))
}

/// Tokens that select more than one dialect.
///
/// A dialect is named one-to-one now, so the pairs that used to share a name
/// (`javascript` covering `.mjs`, `jsx` covering `.tsx`) have to be stated.
const GROUPS: &[(&str, &[&str])] = &[
    ("javascript", &["javascript", "mjs"]),
    ("jsx", &["jsx", "tsx"]),
];

/// Every extension a `--type` token stands for.
pub fn extensions_for_token(token: &str) -> Option<Vec<&'static str>> {
    let language = for_token(token)?;
    let members: &[&str] = GROUPS
        .iter()
        .find_map(|(name, group)| (*name == language.name).then_some(*group))
        .unwrap_or(&[]);
    let mut extensions: Vec<&'static str> = LANGUAGES
        .iter()
        .filter(|found| found.name == language.name || members.contains(&found.name))
        .flat_map(|found| found.extensions.iter().copied())
        .collect();
    extensions.sort_unstable();
    extensions.dedup();
    Some(extensions)
}

/// Every extension read, space separated, for the unknown-type diagnostic.
pub fn known() -> String {
    let mut every: Vec<&str> = LANGUAGES
        .iter()
        .flat_map(|language| language.extensions.iter().copied())
        .collect();
    every.sort_unstable();
    every.dedup();
    every.join(" ")
}

