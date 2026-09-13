use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::catalog::{self, Catalog};
use super::{Language, Options};

#[derive(Clone)]
pub(super) struct Task {
    pub name: String,
    pub directory: PathBuf,
    pub program: &'static str,
    pub arguments: Vec<OsString>,
    pub workers: usize,
    pub cargo: bool,
    pub prepare: bool,
    pub max_workers: usize,
    pub requires: Vec<String>,
    pub output: Option<PathBuf>,
    pub environment: Vec<(OsString, OsString)>,
}

impl Task {
    pub fn nextest(&self) -> bool {
        self.program == "cargo" && self.arguments.first().is_some_and(|arg| arg == "nextest")
    }
    pub fn suite(&self) -> &str {
        self.name
            .match_indices(' ')
            .nth(1)
            .map_or(&self.name, |(end, _)| &self.name[..end])
    }

    fn new(
        name: impl Into<String>,
        directory: PathBuf,
        program: &'static str,
        arguments: &[&str],
        max_workers: usize,
    ) -> Self {
        Self {
            name: name.into(),
            directory,
            program,
            arguments: arguments.iter().map(OsString::from).collect(),
            workers: 1,
            cargo: false,
            prepare: false,
            max_workers,
            requires: Vec::new(),
            output: None,
            environment: Vec::new(),
        }
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(self.program);
        let workers = self.workers.to_string();
        let nextest = self.nextest() && self.arguments.get(1).is_some_and(|arg| arg == "run");
        let pytest =
            self.program == "uv" && self.arguments.get(2).is_some_and(|arg| arg == "pytest");
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
            .envs(self.environment.iter().cloned());
        if nextest {
            command.args(["--test-threads", &workers]);
        } else if pytest {
            command.args([
                "--numprocesses",
                if self.workers == 1 { "0" } else { &workers },
                "--dist",
                "worksteal",
            ]);
        }
        let inner = if nextest || pytest { "1" } else { &workers };
        command
            .env("CARGO_BUILD_JOBS", inner)
            .env("RUST_TEST_THREADS", inner)
            .env("RAYON_NUM_THREADS", inner)
            .env("BIOME_THREADS", inner)
            .env("GOMAXPROCS", inner);
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
        let redirect = self.output.as_ref().map_or_else(String::new, |path| {
            format!(" > {}", quote(path.as_os_str()))
        });
        format!(
            "(cd {} && {environment} {}{arguments}{redirect})",
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
    directory: &Path,
) -> Result<Vec<Task>, String> {
    let workers = usize::MAX;
    let selected = |lang| options.languages.is_empty() || options.languages.contains(&lang);
    let selected_package = |name: &str| {
        catalog.selected(name)
            && (options.packages.is_empty() || options.packages.iter().any(|target| target == name))
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
                    catalog.selected(&package.name)
                        && (options.packages.is_empty()
                            || options
                                .packages
                                .iter()
                                .any(|target| package.matches(target)))
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
                if options.packages.is_empty() && catalog.affected.is_none() {
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
                .filter(|name| {
                    catalog.selected(name)
                        && (options.packages.is_empty()
                            || options
                                .packages
                                .iter()
                                .any(|target| super::suites::matches(name, target, catalog)))
                })
                .collect::<Vec<_>>();
            if !packages.is_empty() {
                let mut task = Task::new(
                    format!("python {action}"),
                    root.join("scripts/python"),
                    "uv",
                    &["run", "--locked", if is_test { "pytest" } else { "ruff" }],
                    if is_test {
                        usize::from(options.python_workers)
                    } else {
                        1
                    },
                );
                if !is_test {
                    task.arguments.push("check".into());
                }
                if options.packages.is_empty()
                    && (catalog.affected.is_none()
                        || (!is_test && packages.len() == catalog.python.len()))
                {
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
                    let mut task = Task::new("javascript test", plugin, "node", &["--test"], 1);
                    task.arguments.extend(files);
                    tasks.push(task);
                }
            }
            let nvim = "shared/nvim/tests/shell.lua";
            if selected(Language::Lua) && selected_package("nvim") && root.join(nvim).is_file() {
                tasks.push(Task::new(
                    "lua test nvim",
                    root.to_path_buf(),
                    "nvim",
                    &["--headless", "-u", "NONE", "-i", "NONE", "-n", "-l", nvim],
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
        return if options.changed.is_some() {
            Ok(tasks)
        } else {
            Err("no tasks match the selection".into())
        };
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
    prepare(root, catalog, options, directory, &mut tasks);
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

fn prepare(
    root: &Path,
    catalog: &Catalog,
    options: &Options,
    directory: &Path,
    tasks: &mut Vec<Task>,
) {
    let mut additions = Vec::new();
    for task in tasks.iter_mut() {
        if task.name == "rust lint" {
            task.prepare = true;
        }
        if task.name == "rust test" && options.arguments.is_empty() {
            let metadata = directory.join("rust-binaries.json");
            let mut build = task.clone();
            build.name = "rust build".into();
            build.prepare = true;
            build.arguments.retain(|arg| arg != "--no-fail-fast");
            build.arguments.splice(
                0..1,
                [
                    "nextest",
                    "list",
                    "--list-type",
                    "binaries-only",
                    "--message-format",
                    "json",
                ]
                .map(OsString::from),
            );
            build.output = Some(metadata.clone());
            task.requires.push(build.name.clone());
            additions.push(build);
            if catalog.rust.iter().any(|package| {
                package.library
                    && (task.arguments.iter().any(|arg| arg == "--workspace")
                        || task
                            .arguments
                            .iter()
                            .any(|arg| arg == package.name.as_str()))
            }) {
                let mut docs = task.clone();
                docs.name = "rust test doctests".into();
                docs.max_workers = 1;
                docs.arguments.insert(1, "--doc".into());
                additions.push(docs);
            }
            task.cargo = false;
            task.arguments = [
                "nextest",
                "run",
                "--no-fail-fast",
                "--no-tests",
                "pass",
                "--binaries-metadata",
            ]
            .map(OsString::from)
            .to_vec();
            task.arguments.push(metadata.into_os_string());
        }
        if task.name == "python test" {
            let selected = |group: &str| {
                task.arguments
                    .iter()
                    .any(|arg| arg == format!("tests/{group}").as_str())
                    || (task.arguments.iter().any(|arg| arg == "tests")
                        && catalog.python.iter().any(|name| name == group))
            };
            let packages = super::suites::DEPENDENCIES
                .iter()
                .filter(|(suite, _)| selected(suite))
                .flat_map(|(_, packages)| packages.iter().copied())
                .filter(|name| catalog.rust.iter().any(|package| package.name == *name))
                .collect::<std::collections::BTreeSet<_>>();
            if !packages.is_empty() {
                let metadata = directory.join("python-binaries.jsonl");
                let mut build = Task::new(
                    "python build",
                    root.join("scripts/rust"),
                    "cargo",
                    &["build", "--locked", "--bins", "--message-format=json"],
                    usize::MAX,
                );
                for name in packages {
                    build.arguments.extend(["--package".into(), name.into()]);
                }
                build.cargo = true;
                build.prepare = true;
                build.output = Some(metadata.clone());
                task.requires.push(build.name.clone());
                task.environment.push((
                    "DOTFILE_DEV_BUILD_MANIFEST".into(),
                    metadata.into_os_string(),
                ));
                additions.push(build);
            }
        }
    }
    tasks.extend(additions);
    tasks.sort_by_key(|task| (!task.prepare, task.max_workers));
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
            .filter(|file| catalog.selected_file(file))
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
            Language::Shell => ("shuck", &["check", "--output-format", "concise"]),
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
            task.arguments.push(file.as_os_str().to_owned());
        }
        if task.arguments.len() > arguments.len() {
            tasks.push(task);
        }
    }
}
