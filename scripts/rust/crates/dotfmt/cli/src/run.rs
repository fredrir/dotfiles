use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use dotfmt::render::{self, Mode, Row};
use dotfmt_core::config::{Effective, Language, Resolver};
use rayon::prelude::*;
use workstation::Style;

use crate::args::Cli;
use crate::engine::Engine;

const WORKER_STACK: usize = 32 * 1024 * 1024;

type Engines = HashMap<(usize, Language), Arc<Engine>>;

struct Job {
    path: PathBuf,
    language: Language,
    engine: Arc<Engine>,
}

pub fn run(mut cli: Cli) -> Result<ExitCode, String> {
    cli.languages.sort();
    cli.languages.dedup();
    if cli.add || cli.sync {
        return crate::placement::run(&cli);
    }
    if cli.owns {
        return owns(&cli);
    }
    if cli.editor && cli.targets.iter().any(|path| path != Path::new("-")) {
        return Err("--editor reads stdin; use --stdin FILENAME for per-file settings".into());
    }
    let streamed = cli.stdin.is_some()
        || cli.editor
        || cli.targets.iter().any(|path| path == Path::new("-"))
        || (cli.targets.is_empty() && !cli.check && !io::stdin().is_terminal());
    let stream_result = if streamed {
        if cli
            .targets
            .iter()
            .filter(|path| *path == Path::new("-"))
            .count()
            > 1
        {
            return Err("standard input may only be read once".into());
        }
        let result = through(&cli);
        cli.targets.retain(|path| path != Path::new("-"));
        if cli.targets.is_empty() {
            return result;
        }
        Some(result)
    } else {
        None
    };
    if cli.targets.is_empty() && !cli.check {
        workstation::cli::command::<Cli>()
            .print_help()
            .map_err(|error| error.to_string())?;
        println!();
        return Ok(ExitCode::SUCCESS);
    }
    if cli.targets.is_empty() {
        cli.targets.push(PathBuf::from("."));
    }
    let parallel = cli.targets.len() > 1 || cli.targets.iter().any(|path| path.is_dir());
    let result = if parallel {
        let workers = std::env::var("RAYON_NUM_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|workers| *workers > 0)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(1, usize::from)
                    .min(32)
            });
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .stack_size(WORKER_STACK)
            .build()
            .map_err(|error| format!("worker pool: {error}"))?
            .install(|| files(&cli, true, streamed))
    } else {
        with_stack(|| files(&cli, false, streamed))
    };
    match stream_result {
        Some(Err(error)) => {
            if let Err(files_error) = result {
                return Err(format!("{error}\ndotfmt: {files_error}"));
            }
            Err(error)
        }
        Some(Ok(status)) if status != ExitCode::SUCCESS => result.map(|_| ExitCode::FAILURE),
        _ => result,
    }
}

fn with_stack<T: Send>(work: impl FnOnce() -> Result<T, String> + Send) -> Result<T, String> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("dotfmt".into())
            .stack_size(WORKER_STACK)
            .spawn_scoped(scope, work)
            .map_err(|error| format!("formatting worker: {error}"))?
            .join()
            .map_err(|_| "formatting worker panicked".to_string())?
    })
}

fn requested(cli: &Cli, language: Language) -> bool {
    cli.languages.is_empty() || cli.languages.contains(&language)
}

fn forced(cli: &Cli) -> Option<Language> {
    (cli.languages.len() == 1).then(|| cli.languages[0])
}

fn ensure_requested(cli: &Cli, config: &Effective) -> Result<(), String> {
    if let Some(language) = forced(cli)
        && !config
            .languages
            .get(&language)
            .is_some_and(|config| config.enabled)
    {
        return Err(format!(
            "{} is not configured; add {} {{}} to dotfmt.dotfile or install a configuration with --add",
            language.name(),
            language.name()
        ));
    }
    Ok(())
}

fn ensure_dialect(cli: &Cli, languages: impl IntoIterator<Item = Language>) -> Result<(), String> {
    if cli
        .dialect
        .as_deref()
        .is_none_or(|value| value.eq_ignore_ascii_case("auto"))
    {
        return Ok(());
    }
    let unique: BTreeSet<_> = languages.into_iter().collect();
    if unique.len() != 1 {
        return Err("--dialect requires one selected language; use --lang LANGUAGE".into());
    }
    Ok(())
}

fn engine(
    config: &Arc<Effective>,
    language: Language,
    cli: &Cli,
    cache: &mut Engines,
) -> Result<Arc<Engine>, String> {
    let key = (Arc::as_ptr(config) as usize, language);
    if let Some(found) = cache.get(&key) {
        return Ok(Arc::clone(found));
    }
    let found = Arc::new(Engine::new(
        language,
        config.settings(language),
        cli.dialect.as_deref(),
    )?);
    cache.insert(key, Arc::clone(&found));
    Ok(found)
}

fn through(cli: &Cli) -> Result<ExitCode, String> {
    let path = cli
        .stdin
        .as_deref()
        .or_else(|| forced(cli).map(crate::placement::default_stdin))
        .ok_or("standard input needs --stdin FILENAME or one --lang LANGUAGE")?;
    let resolver = Resolver::new();
    let config = resolver.for_file(path)?;
    ensure_requested(cli, &config)?;
    if !config
        .languages
        .iter()
        .any(|(language, config)| config.enabled && requested(cli, *language))
    {
        return Err(
            "no configured languages; add language blocks to dotfmt.dotfile or use --add".into(),
        );
    }
    let language = config
        .select_languages(path, &cli.languages, forced(cli))?
        .filter(|language| requested(cli, *language));
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("stdin: {error}"))?;
    let Some(language) = language else {
        if !cli.check {
            io::stdout()
                .write_all(&input)
                .map_err(|error| format!("stdout: {error}"))?;
        }
        return Ok(ExitCode::SUCCESS);
    };
    ensure_dialect(cli, [language])?;
    let engine = Engine::new(language, config.settings(language), cli.dialect.as_deref())?;
    let output = if language == Language::Lua {
        with_stack(|| engine.format(path, &input, cli.editor))?
    } else {
        engine.format(path, &input, cli.editor)?
    };
    if let Some(repairs) = output.repairs.filter(|_| !cli.quiet) {
        eprintln!(
            "dotfmt: {}: {} {repairs}",
            path.display(),
            if cli.check { "would fix" } else { "fixed" }
        );
    }
    let changed = output.text.as_bytes() != input;
    if !cli.check {
        io::stdout()
            .write_all(output.text.as_bytes())
            .map_err(|error| format!("stdout: {error}"))?;
    }
    Ok(if cli.check && changed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn files(cli: &Cli, parallel: bool, streamed: bool) -> Result<ExitCode, String> {
    let resolver = Resolver::new();
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    let mut visited = HashSet::new();
    let mut configured = BTreeSet::new();
    let mut unreadable = 0;
    // Outer roots cover nested directory targets, so overlapping requests only walk once.
    let mut targets: Vec<_> = cli
        .targets
        .iter()
        .map(|path| {
            resolver
                .absolute_path(path)
                .map(|absolute| (path, absolute))
        })
        .collect::<Result<_, _>>()?;
    targets.sort_by_key(|(_, absolute)| absolute.components().count());
    let mut roots = Vec::new();
    for (target, absolute) in targets {
        let is_dir = target.is_dir();
        if is_dir
            && roots
                .iter()
                .any(|root: &PathBuf| absolute.starts_with(root))
        {
            continue;
        }
        if is_dir {
            roots.push(absolute);
            match resolver.for_directory(target) {
                Ok(config) => configured.extend(
                    config
                        .languages
                        .iter()
                        .filter(|(_, config)| config.enabled)
                        .map(|(language, _)| *language),
                ),
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            }
        }
        match dotfmt_core::walk::gather(
            target,
            dotfmt_core::walk::Symlinks::Report,
            dotfmt_core::walk::Explicit::Regular,
            |_| true,
        ) {
            Ok(found) => {
                unreadable += found.unreadable;
                candidates.extend(found.files.into_iter().map(|path| (path, !is_dir)));
            }
            Err(error) => errors.push(error),
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)));
    let mut selected = Vec::new();
    for (path, explicit) in candidates {
        if !path.is_file() {
            continue;
        }
        let config = match resolver.for_file(&path) {
            Ok(config) => config,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        configured.extend(
            config
                .languages
                .iter()
                .filter(|(_, config)| config.enabled)
                .map(|(language, _)| *language),
        );
        if explicit && let Err(error) = ensure_requested(cli, &config) {
            errors.push(format!("{}: {error}", path.display()));
            continue;
        }
        let language = match config.select_languages(
            &path,
            &cli.languages,
            if explicit { forced(cli) } else { None },
        ) {
            Ok(Some(language)) if requested(cli, language) => language,
            Ok(_) => continue,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let canonical = match fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if !visited.insert(canonical) {
            continue;
        }
        selected.push((path, language, config));
    }
    for language in &cli.languages {
        if !configured.contains(language) {
            errors.push(format!("{} is not configured; add {} {{}} to dotfmt.dotfile or install a configuration with --add", language.name(), language.name()));
        }
    }
    if configured.is_empty() && errors.is_empty() {
        return Err(
            "no configured languages; add language blocks to dotfmt.dotfile or use --add".into(),
        );
    }
    ensure_dialect(
        cli,
        selected
            .iter()
            .map(|(_, language, _)| *language)
            .chain(cli.languages.iter().copied()),
    )?;
    let mut engines = Engines::new();
    let mut jobs = Vec::new();
    for (path, language, config) in selected {
        match engine(&config, language, cli, &mut engines) {
            Ok(engine) => jobs.push(Job {
                path,
                language,
                engine,
            }),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    if !errors.is_empty() {
        errors.sort();
        errors.dedup();
        return Err(errors.join("\ndotfmt: "));
    }
    let outcomes: Vec<_> = if parallel {
        jobs.par_iter().map(|job| apply(job, cli.check)).collect()
    } else {
        jobs.iter().map(|job| apply(job, cli.check)).collect()
    };
    let mut rows = BTreeMap::new();
    let mut failed = !errors.is_empty() || unreadable > 0;
    for (job, result) in jobs.iter().zip(outcomes) {
        let row = rows.entry(job.language).or_insert_with(|| Row {
            name: job.language.name(),
            ..Row::default()
        });
        row.files += 1;
        row.ran += 1;
        match result {
            Ok(changed) => {
                if changed && cli.check {
                    row.findings = true;
                    row.blamed.push(job.path.display().to_string());
                    failed = true;
                }
                if cli.verbose {
                    row.output.push_str(&format!(
                        "  {}: {}\n",
                        job.path.display(),
                        if changed {
                            if cli.check {
                                "would reformat"
                            } else {
                                "formatted"
                            }
                        } else {
                            "unchanged"
                        }
                    ));
                }
            }
            Err(error) => {
                row.failed = true;
                row.blamed.push(job.path.display().to_string());
                failed = true;
                errors.push(format!("{}: {error}", job.path.display()));
            }
        }
    }
    errors.sort();
    errors.dedup();
    for error in errors {
        eprintln!("dotfmt: {error}");
    }
    if unreadable > 0 {
        eprintln!("dotfmt: {unreadable} unreadable directories");
    }
    if !cli.quiet && (!streamed || cli.verbose) {
        let rows: Vec<_> = rows.into_values().collect();
        let style = Style::for_stderr();
        let mode = if cli.check { Mode::Check } else { Mode::Write };
        if cli.verbose {
            for line in render::heading(
                "dotfmt",
                &cli.targets[0],
                if cli.check { "check" } else { "" },
                &style,
            ) {
                eprintln!("{line}");
            }
            for line in render::report(&rows, mode, &style) {
                eprintln!("{line}");
            }
            eprintln!("\n  {}", render::tally(&rows, mode));
        } else {
            for line in render::summary(&rows, mode, &style) {
                eprintln!("{line}");
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn apply(job: &Job, check: bool) -> Result<bool, String> {
    let input = fs::read(&job.path).map_err(|error| error.to_string())?;
    let output = job.engine.format(&job.path, &input, false)?;
    let changed = output.text.as_bytes() != input;
    if changed && !check {
        dotfmt_core::file::replace(&job.path, output.text.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    Ok(changed)
}

fn owns(cli: &Cli) -> Result<ExitCode, String> {
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("stdin: {error}"))?;
    let resolver = Resolver::new();
    let mut output = Vec::new();
    for name in input
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let path = path_from_bytes(name)?;
        let config = resolver.for_file(&path)?;
        if config
            .select_languages(&path, &cli.languages, None)?
            .is_some_and(|language| requested(cli, language))
        {
            output.extend_from_slice(name);
            output.push(0);
        }
    }
    io::stdout()
        .write_all(&output)
        .map_err(|error| format!("stdout: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| "filename is not UTF-8".into())
    }
}
