use std::path::Path;

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Write,
    Check,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feed {
    Files,
    Manifests,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drift {
    Status,
    Listing,
}

const QUIET_RUST_LOG: &[(&str, &str)] = &[("RUST_LOG", "warn")];

const ALLOW_COMMENTS: &str = "--json-parse-allow-comments=true";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub program: &'static str,
    pub args: &'static [&'static str],
    pub env: &'static [(&'static str, &'static str)],
    pub feed: Feed,
    pub drift: Drift,
}

impl Step {
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

    pub fn of(path: &Path) -> Option<Lang> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        LANGS
            .into_iter()
            .find(|lang| lang.extensions().contains(&extension.as_str()))
    }

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

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Configured {
    Named,
    Found(&'static str),
    Nothing,
    Gap(&'static str),
}

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
