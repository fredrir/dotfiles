#!/usr/bin/env python3
"""Regenerate `core/src/lang.rs` from the descriptor tables in `core/src/syntax/`.

`lang.rs` is generated, not written: `Dialect`, `Dialect::ALL`, `LANGUAGES` and
`Dialect::syntax()` all come from one parse of `syntax/*.rs`, so they cannot
disagree with each other or with the tables. Adding a language is one `Syntax`
static in one table plus one `ORDER`/`VARIANT` line here, then:

    python3 scripts/rust/crates/utility/doc-purge/core/generate-lang.py

The output is rustfmt-clean by construction (extension lists are pre-wrapped),
so `cargo fmt -p doc-purge-core -- --check` stays green after a regeneration.

It fails loudly rather than guessing: an unknown descriptor name, a name missing
from `ORDER`, or a duplicate name each abort with the offending name.
"""

import re
import pathlib

SRC = pathlib.Path("/home/fredrir/dotfiles/scripts/rust/crates/utility/doc-purge/core/src")
SYNTAX = SRC / "syntax"

FILES = ["curly.rs", "hash.rs", "dash.rs", "config.rs", "markup.rs", "template.rs",
         "script.rs", "style.rs", "shell_ruby.rs"]

VARIANT = {
    "jsonc": "Jsonc", "json5": "Json5", "rust": "Rust", "go": "Go", "c": "C",
    "cpp": "Cpp", "java": "Java", "csharp": "CSharp", "kotlin": "Kotlin",
    "swift": "Swift", "zig": "Zig", "dart": "Dart", "scala": "Scala",
    "groovy": "Groovy", "solidity": "Solidity", "objective-c": "ObjectiveC",
    "vala": "Vala", "d": "D", "haxe": "Haxe", "verilog": "Verilog",
    "wgsl": "Wgsl", "glsl": "Glsl", "gleam": "Gleam", "odin": "Odin", "v": "V",
    "roc": "Roc", "lua": "Lua", "sql": "Sql", "haskell": "Haskell", "ada": "Ada",
    "vhdl": "Vhdl", "fortran": "Fortran", "fortran-fixed": "FortranFixed",
    "matlab": "Matlab", "elm": "Elm", "purescript": "PureScript",
    "ocaml": "Ocaml", "fsharp": "FSharp", "sml": "Sml", "clojure": "Clojure",
    "scheme": "Scheme", "racket": "Racket", "common-lisp": "CommonLisp",
    "emacs-lisp": "EmacsLisp", "assembly": "Assembly", "viml": "Viml",
    "python": "Python", "toml": "Toml", "yaml": "Yaml", "elixir": "Elixir",
    "crystal": "Crystal", "nim": "Nim", "perl": "Perl", "r": "R",
    "julia": "Julia", "powershell": "PowerShell", "tcl": "Tcl", "fish": "Fish",
    "awk": "Awk", "gdscript": "Gdscript", "dockerfile": "Dockerfile",
    "makefile": "Makefile", "cmake": "Cmake", "ini": "Ini", "hcl": "Hcl",
    "starlark": "Starlark", "justfile": "Justfile", "html": "Html",
    "xhtml": "Xhtml", "xml": "Xml", "svg": "Svg", "markdown": "Markdown",
    "mdx": "Mdx", "vue": "Vue", "svelte": "Svelte", "astro": "Astro",
    "xslt": "Xslt", "plist": "Plist", "qml": "Qml", "heex": "Heex", "eex": "Eex",
    "jinja": "Jinja", "twig": "Twig", "handlebars": "Handlebars",
    "mustache": "Mustache", "liquid": "Liquid", "erb": "Erb", "ejs": "Ejs",
    "pug": "Pug", "haml": "Haml", "slim": "Slim", "shell": "Shell",
    "ruby": "Ruby", "javascript": "JavaScript", "mjs": "Mjs",
    "typescript": "TypeScript", "jsx": "Jsx", "tsx": "Tsx", "css": "Css",
    "scss": "Scss", "sass": "Sass", "less": "Less", "stylus": "Stylus",
    "sugarss": "Sugarss",
}

# Extension-less files and shebangs live in the descriptors; order in LANGUAGES
# decides for_path precedence, so the most common claimant of a shared
# extension goes first.
ORDER = [
    "rust", "go", "c", "cpp", "java", "csharp", "kotlin", "swift", "zig", "dart",
    "scala", "groovy", "solidity", "objective-c", "vala", "d", "haxe", "verilog",
    "wgsl", "glsl", "gleam", "odin", "v", "roc",
    "javascript", "mjs", "typescript", "jsx", "tsx", "qml",
    "python", "toml", "yaml", "jsonc", "json5", "ini",
    "shell", "ruby", "elixir", "crystal", "nim", "perl", "r", "julia",
    "powershell", "tcl", "fish", "awk", "gdscript",
    "dockerfile", "makefile", "cmake", "justfile", "starlark", "hcl",
    "lua", "sql", "haskell", "ada", "vhdl", "fortran", "fortran-fixed",
    "matlab", "elm", "purescript", "ocaml", "fsharp", "sml",
    "clojure", "scheme", "racket", "common-lisp", "emacs-lisp", "assembly", "viml",
    "css", "scss", "sass", "less", "stylus", "sugarss",
    "html", "xhtml", "xml", "svg", "xslt", "plist", "markdown", "mdx",
    "vue", "svelte", "astro", "heex", "eex",
    "jinja", "twig", "handlebars", "mustache", "liquid", "erb", "ejs",
    "pug", "haml", "slim",
]



def wrap(items, indent, tail):
    """One wrapped row per ~92 columns, so rustfmt has nothing to change."""
    lines, current = [], ""
    for item in items:
        candidate = f"{current}{item}, " if current else f"{indent}{item}, "
        if len(candidate.rstrip()) > 92 and current:
            lines.append(current.rstrip())
            current = f"{indent}{item}, "
        else:
            current = candidate
    lines.append(current.rstrip())
    return lines


def strip_comments(text):
    out = []
    for line in text.splitlines():
        s = line.split("//")[0] if '"' not in line else line
        out.append(s)
    return "\n".join(out)


def parse(path):
    """Yield (STATIC, name, extensions, filenames, shebangs) per static."""
    text = path.read_text()
    found = {}
    for m in re.finditer(r"^pub static ([A-Z_0-9]+): Syntax = Syntax \{(.*?)^\};",
                         text, re.S | re.M):
        static, body = m.group(1), m.group(2)
        name = re.search(r'^\s*name: "([^"]+)"', body, re.M)
        if not name:
            raise SystemExit(f"{path.name}: {static} has no name")
        def arr(field):
            a = re.search(r"^\s*" + field + r": &\[(.*?)\]", body, re.M | re.S)
            if not a:
                return []
            return re.findall(r'"([^"]*)"', a.group(1))
        found.setdefault(name.group(1), (static, arr("extensions"),
                                         arr("filenames"), arr("shebangs")))
    return found


def main():
    table = {}
    for f in FILES:
        p = SYNTAX / f
        if not p.exists():
            print(f"  (missing {f})")
            continue
        for name, entry in parse(p).items():
            if name in table:
                raise SystemExit(f"duplicate descriptor name: {name}")
            table[name] = entry

    missing_order = [n for n in ORDER if n not in table]
    extra = [n for n in table if n not in ORDER]
    if missing_order:
        print(f"  not yet provided: {missing_order}")
    if extra:
        raise SystemExit(f"descriptors not in ORDER: {extra}")

    names = [n for n in ORDER if n in table]
    for n in names:
        if n not in VARIANT:
            raise SystemExit(f"no Dialect variant for {n}")

    lines = []
    w = lines.append
    w("//! The languages doc-purge reads.")
    w("//!")
    w("//! Generated from `syntax/*.rs`: `Dialect`, `Dialect::ALL`, `LANGUAGES` and")
    w("//! `Dialect::syntax()` all come from one table, so they cannot disagree.")
    w("")
    w("use std::path::Path;")
    w("")
    w("pub use crate::syntax::{Family, Syntax};")
    w("")
    w("use crate::syntax::{config, curly, dash, hash, markup, script, shell_ruby, style, template};")
    w("")
    w("/// One language doc-purge can read.")
    w("///")
    w("/// A closed set: every variant has a descriptor, and `syntax()` is an")
    w("/// exhaustive match, so a new variant does not compile until it has one.")
    w("#[derive(Clone, Copy, PartialEq, Eq, Debug)]")
    w("pub enum Dialect {")
    for n in names:
        w(f"    {VARIANT[n]},")
    w("}")
    w("")
    w("impl Dialect {")
    w("    /// Every dialect, in registry order.")
    w("    pub const ALL: &'static [Dialect] = &[")
    for n in names:
        w(f"        Dialect::{VARIANT[n]},")
    w("    ];")
    w("")
    w("    /// The descriptor behind this dialect.")
    w("    pub fn syntax(self) -> &'static Syntax {")
    w("        match self {")
    for n in names:
        static = table[n][0]
        module = next(f[:-3] for f in FILES if f"pub static {static}: Syntax" in (SYNTAX / f).read_text())
        w(f"            Dialect::{VARIANT[n]} => &{module}::{static},")
    w("        }")
    w("    }")
    w("")
    w("    /// How this language groups with the languages it is lexed like.")
    w("    pub fn family(self) -> crate::syntax::Family {")
    w("        self.syntax().family")
    w("    }")
    w("")
    w("    /// Which engine lexes this language.")
    w("    pub fn engine(self) -> crate::syntax::Engine {")
    w("        self.syntax().engine")
    w("    }")
    w("}")
    w("")
    w("/// A language and the extensions it claims.")
    w("pub struct Language {")
    w("    pub name: &'static str,")
    w("    pub dialect: Dialect,")
    w("    pub extensions: &'static [&'static str],")
    w("}")
    w("")
    w("/// Every language, in the order `for_path` prefers.")
    w("pub const LANGUAGES: &[Language] = &[")
    for n in names:
        _, exts, _, _ = table[n]
        w("    Language {")
        w(f'        name: "{n}",')
        w(f"        dialect: Dialect::{VARIANT[n]},")
        w("        extensions: &[")
        for line in wrap([f'"{e}"' for e in exts], "            ", "        ]"):
            w(line)
        w("        ],")
        w("    },")
    w("];")
    w("")
    w("""/// The extension-only fast path.
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
            } else if uses("%") || uses("function ") || has("end\\n") {
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
        .position(|byte| matches!(byte, b'\\n' | b'\\r'))
        .unwrap_or(head.len());
    &head[..end]
}
""")
    w("const ALIASES: &[(&str, &str)] = &[")
    w('    ("css-module", "css"),')
    w('    ("css-modules", "css"),')
    w('    ("cssmodules", "css"),')
    w('    ("module", "css"),')
    w('    ("tailwind", "css"),')
    w('    ("tailwindcss", "css"),')
    w('    ("node", "javascript"),')
    w('    ("nodejs", "javascript"),')
    w('    ("ecmascript", "javascript"),')
    w('    ("es", "javascript"),')
    w('    ("sass-indented", "sass"),')
    w('    ("indented", "sass"),')
    w('    ("sugar", "sugarss"),')
    w('    ("ts", "typescript"),')
    w('    ("js", "javascript"),')
    w('    ("py", "python"),')
    w('    ("rs", "rust"),')
    w('    ("sh", "shell"),')
    w('    ("bash", "shell"),')
    w('    ("zsh", "shell"),')
    w('    ("yml", "yaml"),')
    w('    ("md", "markdown"),')
    w('    ("hs", "haskell"),')
    w('    ("rb", "ruby"),')
    w('    ("ps1", "powershell"),')
    w('    ("tf", "hcl"),')
    w("];")
    w("")
    w("""/// Resolve a `--type` token: an alias, a language name or an extension.
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
""")

    target = SRC / "lang.rs"
    target.write_text("\n".join(lines) + "\n")
    # rustfmt is not part of `dotfile dev lint`, so format here: the generated
    # file must not reintroduce formatting drift every time it is rebuilt.
    import subprocess
    subprocess.run(
        ["rustfmt", "--edition", "2024", str(target)],
        check=False,
        capture_output=True,
    )
    print(f"  wrote lang.rs with {len(names)} dialects, "
          f"{sum(len(table[n][1]) for n in names)} extensions")


if __name__ == "__main__":
    main()
