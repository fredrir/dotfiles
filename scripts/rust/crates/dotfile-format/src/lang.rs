//! The provider table: which tool owns which language, and in what order its
//! programs run.
//!
//! Languages are modelled rather than tools, because two of the rows need
//! more than one program — `.go` is `goimports` and then `gofmt`, `.yaml` in
//! `--check` is `yamlfmt` and then `yamllint` — and because `--add` seeds one
//! config per language rather than one per program.
//!
//! Every extension belongs to exactly one language, which is what lets the
//! walk sort a file into its row by looking at nothing but the name.

use std::path::Path;

/// One row of the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Dotfmt,
    Python,
    Web,
    Lua,
    Rust,
    Toml,
    Yaml,
    Sql,
    Shell,
    Go,
}

/// Every row, in the order a report lists them.
pub const LANGS: [Lang; 10] = [
    Lang::Dotfmt,
    Lang::Python,
    Lang::Web,
    Lang::Lua,
    Lang::Rust,
    Lang::Toml,
    Lang::Yaml,
    Lang::Sql,
    Lang::Shell,
    Lang::Go,
];

/// What a run is for: rewriting the files, or reporting on them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Run the formatters and let them write.
    Write,
    /// Run the formatters in verify mode and the linters as well, writing
    /// nothing.
    Check,
}

/// What a step is handed to work on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feed {
    /// The file list, relative to the run root, in chunks.
    Files,
    /// One `--manifest-path` per Cargo workspace found under the root. Only
    /// `cargo fmt` wants this: it takes a manifest and formats what the
    /// manifest says, not a list of files.
    Manifests,
}

/// How a step in `--check` says the files are not formatted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drift {
    /// A non-zero exit, which is what all but two of them do.
    Status,
    /// Success either way, with the offending files named on stdout.
    /// `gofmt -l` and `goimports -l` are the two, and reading their status
    /// instead would report a whole Go tree as clean.
    Listing,
}

/// Environment for a tool whose own logging is louder than its findings.
///
/// taplo writes its entire resolved file list at `INFO` on every invocation —
/// 2600 characters for this repository's 43 TOML files, ahead of anything it
/// found. `warn` drops that line and keeps every `ERROR`, which is where its
/// findings are, and leaves both exit codes alone: clean is still 0 and drift
/// is still 1, so `Drift::Status` reads the row the same way.
const QUIET_RUST_LOG: &[(&str, &str)] = &[("RUST_LOG", "warn")];

/// Let biome parse a `.json` file that is really JSONC.
///
/// The value is written out because the flag is `--flag=<true|false>` rather
/// than a bare switch: without it biome reads the first path as the value and
/// fails before it has looked at a single file.
const ALLOW_COMMENTS: &str = "--json-parse-allow-comments=true";

/// One program invocation, before the files it is given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Set on the child. Empty for every tool that is quiet by default.
    pub env: &'static [(&'static str, &'static str)],
    pub feed: Feed,
    pub drift: Drift,
}

impl Step {
    /// Turn a tool's logging down to the level its findings are written at.
    /// Never below it — a quieter tool that has stopped reporting is worse
    /// than a loud one.
    fn quiet(mut self, env: &'static [(&'static str, &'static str)]) -> Step {
        self.env = env;
        self
    }
}

fn on_files(program: &'static str, args: &'static [&'static str]) -> Step {
    Step {
        program,
        args,
        env: &[],
        feed: Feed::Files,
        drift: Drift::Status,
    }
}

fn on_manifests(program: &'static str, args: &'static [&'static str]) -> Step {
    Step {
        program,
        args,
        env: &[],
        feed: Feed::Manifests,
        drift: Drift::Status,
    }
}

fn by_listing(program: &'static str, args: &'static [&'static str]) -> Step {
    Step {
        program,
        args,
        env: &[],
        feed: Feed::Files,
        drift: Drift::Listing,
    }
}

impl Lang {
    /// What the report calls this row.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Dotfmt => "dotfmt",
            Lang::Python => "python",
            Lang::Web => "web",
            Lang::Lua => "lua",
            Lang::Rust => "rust",
            Lang::Toml => "toml",
            Lang::Yaml => "yaml",
            Lang::Sql => "sql",
            Lang::Shell => "shell",
            Lang::Go => "go",
        }
    }

    /// The extensions this row owns, without the dot.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Lang::Dotfmt => &["conf", "config", "dotfile"],
            Lang::Python => &["py", "pyi"],
            Lang::Web => &[
                "js", "jsx", "ts", "tsx", "mjs", "cjs", "css", "html", "json", "jsonc",
            ],
            Lang::Lua => &["lua"],
            Lang::Rust => &["rs"],
            Lang::Toml => &["toml"],
            Lang::Yaml => &["yaml", "yml"],
            Lang::Sql => &["sql"],
            // zsh is deliberately absent. shfmt claims to read it, but on this
            // repository it silently rewrote `$#` to `$` and `${~pattern}` to
            // `${(~)pattern}` -- the first broke the lazygit wrapper, the second
            // stopped a directory glob matching anything, and shfmt then could not
            // re-parse its own output. `-ln zsh` is worse: 18 of 34 files fail.
            // zsh is a different language, so it goes unformatted rather than wrong.
            Lang::Shell => &["sh", "bash"],
            Lang::Go => &["go"],
        }
    }

    /// Which row a path belongs to, or `None` for a file no tool here owns.
    ///
    /// The extension is lowercased first, so a `.JSON` written by some other
    /// machine still reaches biome.
    pub fn of(path: &Path) -> Option<Lang> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        LANGS
            .into_iter()
            .find(|lang| lang.extensions().contains(&extension.as_str()))
    }

    /// The programs to run, in the order they must run in.
    ///
    /// Within one language this is a sequence rather than a set: `goimports`
    /// rewrites imports and `gofmt` rewrites layout, and running both `-w`
    /// passes over the same file at once is a race on the file itself.
    pub fn steps(self, mode: Mode) -> Vec<Step> {
        match (self, mode) {
            (Lang::Dotfmt, Mode::Write) => vec![on_files("dotfmt", &[])],
            (Lang::Dotfmt, Mode::Check) => vec![on_files("dotfmt", &["--check"])],

            (Lang::Python, Mode::Write) => vec![on_files("ruff", &["format"])],
            (Lang::Python, Mode::Check) => vec![
                on_files("ruff", &["format", "--check"]),
                on_files("ruff", &["check"]),
            ],

            // `.json` in this repository is sometimes JSONC — a leading `//`
            // in `shared/vscode/keybindings.json` is what editors write there
            // — and biome refuses to parse one without being told. The flag
            // takes an explicit value: bare `--json-parse-allow-comments`
            // swallows the next argument as its value and the run dies on
            // "provided string was not `true` or `false`".
            (Lang::Web, Mode::Write) => {
                vec![on_files("biome", &["format", "--write", ALLOW_COMMENTS])]
            }
            (Lang::Web, Mode::Check) => vec![
                on_files("biome", &["format", ALLOW_COMMENTS]),
                on_files("biome", &["lint", ALLOW_COMMENTS]),
            ],

            (Lang::Lua, Mode::Write) => vec![on_files("stylua", &[])],
            (Lang::Lua, Mode::Check) => vec![on_files("stylua", &["--check"])],

            (Lang::Rust, Mode::Write) => vec![on_manifests("cargo", &["fmt", "--all"])],
            (Lang::Rust, Mode::Check) => {
                vec![on_manifests("cargo", &["fmt", "--all", "--check"])]
            }

            (Lang::Toml, Mode::Write) => vec![on_files("taplo", &["fmt"]).quiet(QUIET_RUST_LOG)],
            (Lang::Toml, Mode::Check) => vec![
                on_files("taplo", &["fmt", "--check"]).quiet(QUIET_RUST_LOG),
                on_files("taplo", &["lint"]).quiet(QUIET_RUST_LOG),
            ],

            // yamlfmt writes in place with no flag at all. `-w` is not one
            // of its flags: it exits 2 printing the usage text, which is why
            // no YAML in this repository had ever been formatted.
            (Lang::Yaml, Mode::Write) => vec![on_files("yamlfmt", &[])],
            (Lang::Yaml, Mode::Check) => {
                vec![on_files("yamlfmt", &["-lint"]), on_files("yamllint", &[])]
            }

            // sqlfluff has no verify mode that writes nothing and reports
            // drift, so `--check` is its linter alone.
            (Lang::Sql, Mode::Write) => vec![on_files("sqlfluff", &["format"])],
            (Lang::Sql, Mode::Check) => vec![on_files("sqlfluff", &["lint"])],

            (Lang::Shell, Mode::Write) => vec![on_files("shfmt", &["-w"])],
            (Lang::Shell, Mode::Check) => vec![on_files("shfmt", &["-d"])],

            (Lang::Go, Mode::Write) => {
                vec![on_files("goimports", &["-w"]), on_files("gofmt", &["-w"])]
            }
            (Lang::Go, Mode::Check) => {
                vec![
                    by_listing("goimports", &["-l"]),
                    by_listing("gofmt", &["-l"]),
                ]
            }
        }
    }

    /// The file in `shared/tools/` that seeds a project for this language,
    /// and the name it has to land under.
    ///
    /// Only biome differs between the two: the repository keeps its copy as
    /// `biome.global.json` so that linking it into `$HOME` does not shadow a
    /// project's own, but `biome.json` is the only name biome reads.
    pub fn config(self) -> Option<(&'static str, &'static str)> {
        match self {
            Lang::Dotfmt => Some(("dotfmt.dotfile", "dotfmt.dotfile")),
            Lang::Python => Some(("ruff.toml", "ruff.toml")),
            Lang::Web => Some(("biome.global.json", "biome.json")),
            Lang::Lua => Some(("stylua.toml", "stylua.toml")),
            Lang::Rust => Some(("rustfmt.toml", "rustfmt.toml")),
            Lang::Toml => Some((".taplo.toml", ".taplo.toml")),
            Lang::Yaml => Some((".yamllint.yaml", ".yamllint.yaml")),
            Lang::Sql => Some((".sqlfluff", ".sqlfluff")),
            Lang::Shell => Some((".editorconfig", ".editorconfig")),
            // Nothing here configures gofmt; it has no configuration.
            Lang::Go => None,
        }
    }

    /// Files whose presence at the top of a project says the language is used
    /// even when the walk happens to find none of its sources.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            Lang::Python => &["pyproject.toml", "uv.lock"],
            Lang::Web => &["package.json", "tsconfig.json"],
            Lang::Rust => &["Cargo.toml"],
            Lang::Go => &["go.mod"],
            Lang::Lua => &[".luarc.json", "init.lua"],
            _ => &[],
        }
    }
}

/// Where a program's configuration comes from.
///
/// Four of the twelve look for a config where this repository keeps none —
/// the configs live under `shared/tools/`, not at the root. taplo and stylua
/// formatted the whole tree at their own defaults and reported it clean;
/// biome and yamllint are saved here only by a symlink in `$HOME` that a
/// fresh checkout would not have. Each was found by hand, one after another,
/// which is three times too many.
///
/// So every program in the table has to say which case it is, and
/// `every_program_says_where_its_configuration_comes_from` fails a row that
/// has not. A fifth one is a red test rather than a silent wrong-settings run.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Configured {
    /// The run names the config on the command line, because the program
    /// would find nothing where it looks. Must be in `configs::INJECTED`.
    Named,
    /// Left to the program, with the reason that is safe.
    Found(&'static str),
    /// Nothing here configures it.
    Nothing,
    /// A gap that is known, is not closed here, and says why.
    Gap(&'static str),
}

/// Every program the table can run, once each, in the order the rows give.
#[cfg(test)]
pub fn programs() -> Vec<&'static str> {
    let mut named = Vec::new();
    for lang in LANGS {
        for mode in [Mode::Write, Mode::Check] {
            for step in lang.steps(mode) {
                if !named.contains(&step.program) {
                    named.push(step.program);
                }
            }
        }
    }
    named
}

/// What settles this program's configuration, and why that is the right
/// answer for it.
///
/// `None` for a program that has not been asked the question. That is the
/// whole mechanism: a new row whose author did not think about where its
/// config comes from fails the test rather than quietly running at defaults.
#[cfg(test)]
pub fn configured(program: &str) -> Option<Configured> {
    Some(match program {
        "taplo" | "biome" | "stylua" | "yamllint" => Configured::Named,

        // dotfmt resolves per file and falls back to `~/.config/dotfmt/`.
        // That rule is the thing dotfmt exists to own — the same reasoning
        // that made this crate ask `--owns` rather than guess — so naming one
        // config for a whole run would override it everywhere.
        "dotfmt" => Configured::Found("resolves per file and owns that rule"),
        "ruff" => Configured::Found("reads ~/.config/ruff/ruff.toml, which this repository links"),
        "sqlfluff" => Configured::Found("reads ~/.sqlfluff, which this repository links"),
        // shfmt reads `.editorconfig` from the file's own directory upward,
        // and this repository links one into `$HOME`, which is above any
        // target under it.
        "shfmt" => Configured::Found("reads .editorconfig upward, and one is linked into $HOME"),

        // rustfmt does take `--config-path`, but only after the manifest and
        // behind a `--`, which is a different argument position from every
        // other row here. It is a gap rather than a live defect: every value
        // in `shared/tools/rustfmt.toml` is currently rustfmt's own default,
        // so `cargo fmt --check` agrees with and without it. Editing that file
        // would open it silently.
        "cargo" => Configured::Gap("takes its config after the manifest, behind a --"),

        // yamlfmt reads `.yamlfmt` from the working directory. This
        // repository ships no such file, so there is nothing to name.
        "yamlfmt" => Configured::Nothing,
        "gofmt" | "goimports" => Configured::Nothing,

        _ => return None,
    })
}
