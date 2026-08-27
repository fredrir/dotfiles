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
            Lang::Shell => &["sh", "bash", "zsh"],
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

            (Lang::Web, Mode::Write) => vec![on_files("biome", &["format", "--write"])],
            (Lang::Web, Mode::Check) => {
                vec![on_files("biome", &["format"]), on_files("biome", &["lint"])]
            }

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

            (Lang::Yaml, Mode::Write) => vec![on_files("yamlfmt", &["-w"])],
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
            Lang::Dotfmt => Some(("dotfile.dotfile", "dotfile.dotfile")),
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
