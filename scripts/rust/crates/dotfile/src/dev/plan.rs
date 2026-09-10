use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::catalog::{self, Catalog};
use super::{Language, Options};

pub(super) struct Task {
    pub name: String,
    pub directory: PathBuf,
    pub program: &'static str,
    pub arguments: Vec<OsString>,
    pub workers: usize,
    pub cargo: bool,
}

impl Task {
    fn new(
        name: impl Into<String>,
        directory: PathBuf,
        program: &'static str,
        arguments: &[&str],
        workers: usize,
    ) -> Self {
        Self {
            name: name.into(),
            directory,
            program,
            arguments: arguments.iter().map(OsString::from).collect(),
            workers,
            cargo: false,
        }
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(self.program);
        let workers = self.workers.to_string();
        if self.program == "node" {
            command.arg(format!("--test-concurrency={workers}"));
        } else if self.program == "luacheck" {
            command.args(["--jobs", &workers]);
        } else if self.program == "uv" {
            command.env("PYTHONUNBUFFERED", "1");
        }
        command
            .current_dir(&self.directory)
            .args(&self.arguments)
            .env("CARGO_BUILD_JOBS", &workers)
            .env("RUST_TEST_THREADS", &workers)
            .env("RAYON_NUM_THREADS", &workers)
            .env("BIOME_THREADS", &workers);
        command
    }

    pub fn display(&self) -> String {
        let quote = |value: &std::ffi::OsStr| hostkit::shell::quote(&value.to_string_lossy());
        let command = self.command();
        let environment = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| format!("{}={}", key.to_string_lossy(), quote(value)))
            })
            .collect::<Vec<_>>()
            .join(" ");
        let arguments = command
            .get_args()
            .map(|argument| format!(" {}", quote(argument)))
            .collect::<String>();
        format!(
            "(cd {} && {environment} {}{arguments})",
            quote(self.directory.as_os_str()),
            self.program
        )
    }
}

pub(super) fn tasks(
    root: &Path,
    catalog: &Catalog,
    options: &Options,
    test: bool,
    lint: bool,
    workers: usize,
) -> Result<Vec<Task>, String> {
    for target in &options.packages {
        if !catalog.known(root, target, &options.languages) {
            return Err(format!("unknown package '{target}' for selected languages"));
        }
    }
    let selected = |lang| options.languages.is_empty() || options.languages.contains(&lang);
    let selected_package = |name: &str| {
        options.packages.is_empty() || options.packages.iter().any(|target| target == name)
    };
    let mut tasks = Vec::new();
    for is_test in [false, true] {
        if (is_test && !test) || (!is_test && !lint) {
            continue;
        }
        let action = if is_test { "test" } else { "lint" };
        if selected(Language::Rust) {
            let packages = catalog
                .rust
                .iter()
                .filter(|package| {
                    options.packages.is_empty()
                        || options
                            .packages
                            .iter()
                            .any(|target| package.matches(target))
                })
                .collect::<Vec<_>>();
            if !packages.is_empty() {
                let mut task = Task::new(
                    format!("rust {action}"),
                    root.join("scripts/rust"),
                    "cargo",
                    &[if is_test { "test" } else { "clippy" }, "--locked"],
                    workers,
                );
                task.cargo = true;
                if options.packages.is_empty() {
                    task.arguments.push("--workspace".into());
                } else {
                    for package in packages {
                        task.arguments
                            .extend([OsString::from("--package"), package.name.clone().into()]);
                    }
                }
                if !is_test {
                    task.arguments.push("--all-targets".into());
                } else {
                    task.arguments.push("--no-fail-fast".into());
                }
                tasks.push(task);
            }
        }
        if selected(Language::Python) {
            let packages = catalog
                .python
                .iter()
                .filter(|name| selected_package(name))
                .collect::<Vec<_>>();
            if options.packages.is_empty() || !packages.is_empty() {
                let mut task = Task::new(
                    format!("python {action}"),
                    root.join("scripts/python"),
                    "uv",
                    &["run", "--locked", if is_test { "pytest" } else { "ruff" }],
                    workers,
                );
                task.cargo = is_test;
                if !is_test {
                    task.arguments.push("check".into());
                }
                if options.packages.is_empty() {
                    task.arguments
                        .push(if is_test { "tests" } else { "." }.into());
                } else {
                    for name in packages {
                        task.arguments.push(format!("tests/{name}").into());
                        if !is_test {
                            let source = format!("src/tools/{name}");
                            if task.directory.join(&source).is_dir() {
                                task.arguments.push(source.into());
                            }
                        }
                    }
                }
                tasks.push(task);
            }
        }
        if is_test {
            let plugin = root.join("shared/obsidian/plugins/agent-transcripts");
            if selected(Language::Javascript)
                && selected_package("agent-transcripts")
                && plugin.is_dir()
            {
                let mut files = std::fs::read_dir(&plugin)
                    .map_err(|error| error.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .filter(|entry| entry.file_name().to_string_lossy().ends_with(".test.js"))
                    .map(|entry| entry.file_name())
                    .collect::<Vec<_>>();
                files.sort();
                if !files.is_empty() {
                    let mut task =
                        Task::new("javascript test", plugin, "node", &["--test"], workers);
                    task.arguments.extend(files);
                    tasks.push(task);
                }
            }
            let shell = "shared/zsh/tests/tmux.zsh";
            if selected(Language::Shell) && selected_package("zsh") && root.join(shell).is_file() {
                tasks.push(Task::new(
                    "shell test",
                    root.to_path_buf(),
                    "zsh",
                    &["-dfi", shell],
                    1,
                ));
            }
            let script = "shared/wezterm/tests/tmux-workspace.lua";
            if selected(Language::Lua) && selected_package("wezterm") && root.join(script).is_file()
            {
                for platform in ["linux", "mac"] {
                    for mode in ["hwire-splits", "native-splits"] {
                        tasks.push(Task::new(
                            format!("lua test {platform} {mode}"),
                            root.to_path_buf(),
                            "lua",
                            &[script, platform, mode],
                            1,
                        ));
                    }
                }
            }
        } else {
            add_linters(&mut tasks, root, catalog, options, workers);
        }
    }
    if tasks.is_empty() {
        return Err("no tasks match the selection".into());
    }
    if !options.arguments.is_empty() {
        if test && lint {
            return Err("argument forwarding requires test or lint; check runs both".into());
        }
        if tasks.len() != 1 {
            return Err("argument forwarding requires one runner; narrow --lang and --pkg".into());
        }
        tasks[0].arguments.extend(options.arguments.iter().cloned());
    }
    for task in &mut tasks {
        if task.program == "cargo" && task.arguments.first().is_some_and(|arg| arg == "clippy") {
            let split = task
                .arguments
                .iter()
                .position(|arg| arg == "--")
                .unwrap_or(task.arguments.len());
            if split == task.arguments.len() {
                task.arguments.push("--".into());
            }
            task.arguments.splice(
                split + 1..split + 1,
                [OsString::from("-D"), OsString::from("warnings")],
            );
        }
    }
    Ok(tasks)
}

fn add_linters(
    tasks: &mut Vec<Task>,
    root: &Path,
    catalog: &Catalog,
    options: &Options,
    workers: usize,
) {
    for lang in [
        Language::Javascript,
        Language::Lua,
        Language::Shell,
        Language::Toml,
        Language::Yaml,
        Language::Json,
    ] {
        if !options.languages.is_empty() && !options.languages.contains(&lang) {
            continue;
        }
        let files = catalog
            .files
            .iter()
            .filter(|file| catalog::language(file) == Some(lang))
            .filter(|file| {
                options.packages.is_empty()
                    || options
                        .packages
                        .iter()
                        .any(|target| catalog.matches(file, target))
            })
            .collect::<Vec<_>>();
        if files.is_empty() {
            continue;
        }
        let (program, arguments): (&str, &[&str]) = match lang {
            Language::Javascript | Language::Json => (
                "biome",
                &[
                    "lint",
                    "--config-path=shared/tools/biome.global.json",
                    "--json-parse-allow-comments=true",
                ],
            ),
            Language::Lua => ("luacheck", &[]),
            Language::Shell => ("shellcheck", &[]),
            Language::Toml => ("taplo", &["lint", "--config", "shared/tools/.taplo.toml"]),
            Language::Yaml => ("yamllint", &["-c", "shared/tools/.yamllint.yaml"]),
            _ => unreachable!(),
        };
        let mut task = Task::new(
            format!("{} lint", lang.name()),
            root.to_path_buf(),
            program,
            arguments,
            workers,
        );
        for file in files {
            if lang == Language::Shell && catalog::is_zsh(file) {
                let mut syntax = Task::new(
                    format!("shell lint {}", file.display()),
                    root.to_path_buf(),
                    "zsh",
                    &["-n"],
                    1,
                );
                syntax.arguments.push(file.as_os_str().to_owned());
                tasks.push(syntax);
            } else {
                task.arguments.push(file.as_os_str().to_owned());
            }
        }
        if task.arguments.len() > arguments.len() {
            tasks.push(task);
        }
    }
}
