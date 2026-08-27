//! Handing the files to the tools that own them.
//!
//! Languages run in parallel and the steps within one language run in order.
//! That split is not an optimisation, it is a correctness rule: `goimports`
//! and `gofmt` both rewrite the same `.go` file, so letting the two `-w`
//! passes overlap is a race on the file itself. Two different languages can
//! never be handed the same path, so there is nothing to race between them.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use rayon::prelude::*;

use crate::lang::{Drift, Feed, LANGS, Lang, Mode, Step};

/// How many paths go on one command line. Well under any `ARG_MAX`, and a
/// tree big enough to need several chunks is rare enough that the extra
/// process launches cost nothing worth measuring.
pub const CHUNK: usize = 512;

/// What one language's share of a run came to.
pub struct Ran {
    pub lang: Lang,
    /// How many files this language owned.
    pub files: usize,
    /// Programs that are not on `PATH`. Never a failure, only a fact.
    pub missing: Vec<&'static str>,
    /// A tool reported drift or a lint violation.
    pub findings: bool,
    /// A tool could not do its job at all.
    pub failed: bool,
    /// How many programs actually ran, so a row where every program is
    /// missing can say so rather than claim success.
    pub ran: usize,
    /// Something worth saying that no tool said.
    pub note: Option<String>,
    /// What the tools themselves said, verbatim.
    pub output: String,
}

impl Ran {
    fn new(lang: Lang, files: usize) -> Ran {
        Ran {
            lang,
            files,
            missing: Vec::new(),
            findings: false,
            failed: false,
            ran: 0,
            note: None,
            output: String::new(),
        }
    }
}

/// Sort the candidates into their rows, dropping the rows with nothing in
/// them.
pub fn sort(files: Vec<PathBuf>) -> Vec<(Lang, Vec<PathBuf>)> {
    let mut work: Vec<(Lang, Vec<PathBuf>)> =
        LANGS.into_iter().map(|lang| (lang, Vec::new())).collect();
    for file in files {
        if let Some(lang) = Lang::of(&file) {
            let slot = work
                .iter_mut()
                .find(|(row, _)| *row == lang)
                .expect("every language has a row");
            slot.1.push(file);
        }
    }
    work.retain(|(_, files)| !files.is_empty());
    work
}

/// `root` must be absolute: it is both the working directory every child is
/// given and the base the relative paths are resolved against.
pub fn run(root: &Path, work: Vec<(Lang, Vec<PathBuf>)>, mode: Mode, verbose: bool) -> Vec<Ran> {
    let mut done: Vec<Ran> = work
        .into_par_iter()
        .map(|(lang, files)| one(root, lang, &files, mode, verbose))
        .collect();
    // Back into table order, so two runs over the same tree read the same.
    done.sort_by_key(|ran| LANGS.iter().position(|lang| *lang == ran.lang));
    done
}

fn one(root: &Path, lang: Lang, files: &[PathBuf], mode: Mode, verbose: bool) -> Ran {
    let mut ran = Ran::new(lang, files.len());
    for step in lang.steps(mode) {
        let batches = match step.feed {
            Feed::Files => batches(files),
            Feed::Manifests => manifest_arguments(root, files),
        };
        // Rust files with no manifest over them are nothing cargo can be
        // asked about, which is worth saying rather than passing over.
        if step.feed == Feed::Manifests && batches.is_empty() {
            ran.note = Some("no Cargo.toml above these files".to_string());
            continue;
        }
        // The step is named once however many command lines it took, so a
        // section opens with the finding rather than with the file list.
        let mut announced = false;
        for batch in batches {
            match invoke(root, &step, &batch) {
                Ok(output) => {
                    ran.ran += 1;
                    absorb(
                        &mut ran,
                        &step,
                        &batch,
                        &output,
                        mode,
                        Told {
                            verbose,
                            total: files.len(),
                        },
                        &mut announced,
                    );
                }
                // A tool nobody installed is a fact about this machine, not a
                // failure of this run, and the rest of the languages still
                // have their tools.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    ran.missing.push(step.program);
                    break;
                }
                Err(error) => {
                    ran.failed = true;
                    ran.output.push_str(&format!("{}: {error}\n", step.program));
                    break;
                }
            }
        }
    }
    ran
}

/// The file list, split into command lines. Chunks of one step run in order
/// rather than at once: they are disjoint, but two of them writing at once is
/// a shape this driver deliberately never takes.
pub fn batches(files: &[PathBuf]) -> Vec<Vec<OsString>> {
    files
        .chunks(CHUNK)
        .map(|chunk| chunk.iter().map(|path| path.as_os_str().into()).collect())
        .collect()
}

/// Every child runs in the root with relative paths, so taplo finds
/// `.taplo.toml`, biome finds `biome.json` and stylua finds `stylua.toml` by
/// the upward search each of them already does. `stdin` is closed because a
/// tool that decided to read from the terminal would hang a run that has no
/// business being interactive.
pub fn invoke(root: &Path, step: &Step, trailing: &[OsString]) -> io::Result<Output> {
    Command::new(step.program)
        .args(step.args)
        .args(trailing)
        .envs(step.env.iter().copied())
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
}

/// How much of a command to show, and what the whole step was handed.
#[derive(Clone, Copy)]
struct Told {
    verbose: bool,
    total: usize,
}

fn absorb(
    ran: &mut Ran,
    step: &Step,
    batch: &[OsString],
    output: &Output,
    mode: Mode,
    told: Told,
    announced: &mut bool,
) {
    let ok = output.status.success();
    // `gofmt -l` and `goimports -l` succeed whether or not the files are
    // formatted and name the unformatted ones on stdout, so their status says
    // nothing about drift and their silence is the only thing that does.
    let listed = mode == Mode::Check
        && step.drift == Drift::Listing
        && !String::from_utf8_lossy(&output.stdout).trim().is_empty();
    match mode {
        Mode::Write => ran.failed |= !ok,
        Mode::Check => {
            ran.findings |= listed;
            if !ok {
                match step.drift {
                    Drift::Status => ran.findings = true,
                    // A listing step reports drift by listing; a non-zero exit
                    // from one is the file failing to parse.
                    Drift::Listing => ran.failed = true,
                }
            }
        }
    }
    if !(told.verbose || !ok || listed) {
        return;
    }
    // Under `--verbose` every invocation is named in full, because seeing
    // exactly what ran is what the flag is for. Otherwise the file list stands
    // in as a count: a check over 194 files would open with 194 paths on one
    // line, and the findings underneath are what a person came for. A manifest
    // is named either way — one manifest is one workspace, and a finding is
    // only reproducible if it says which.
    match (told.verbose, step.feed) {
        (true, _) | (_, Feed::Manifests) => ran.output.push_str(&full(step, batch)),
        (false, Feed::Files) => {
            if !*announced {
                ran.output.push_str(&short(step, told.total));
                *announced = true;
            }
        }
    }
    ran.output.push_str(&said(output));
}

/// The program and its flags, with the file list left out.
fn short(step: &Step, total: usize) -> String {
    let flags = step.args.join(" ");
    let space = if flags.is_empty() { "" } else { " " };
    format!(
        "$ {}{space}{flags} … ({total} {})\n",
        step.program,
        if total == 1 { "file" } else { "files" }
    )
}

/// The command exactly as it was run, argument for argument.
fn full(step: &Step, batch: &[OsString]) -> String {
    format!(
        "$ {} {}\n",
        step.program,
        step.args
            .iter()
            .map(|argument| (*argument).to_string())
            .chain(batch.iter().map(|path| path.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// Whatever the tool had to say, verbatim. This is the part a finding is
/// actionable from, so nothing here is abbreviated.
fn said(output: &Output) -> String {
    let mut text = String::new();
    for stream in [&output.stdout, &output.stderr] {
        let spoken = String::from_utf8_lossy(stream);
        if !spoken.trim().is_empty() {
            text.push_str(spoken.trim_end());
            text.push('\n');
        }
    }
    text
}

/// `cargo fmt` takes a manifest rather than a file list, so the Rust row asks
/// a different question of the same files: which workspaces do they live in?
///
/// The shallowest ancestor holding a `Cargo.toml` is the workspace root, which
/// is why `--all` there formats the whole workspace whatever the target was —
/// a run aimed at one crate of this repository formats every crate in it.
pub fn manifest_arguments(root: &Path, files: &[PathBuf]) -> Vec<Vec<OsString>> {
    workspaces(root, files)
        .into_iter()
        .map(|manifest| vec!["--manifest-path".into(), manifest.into_os_string()])
        .collect()
}

fn workspaces(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = BTreeSet::new();
    for file in files {
        let mut holder = root.join(file);
        holder.pop();
        let mut shallowest = None;
        let mut at = holder.as_path();
        loop {
            if at.join("Cargo.toml").is_file() {
                shallowest = Some(at);
            }
            match at.parent() {
                Some(up) => at = up,
                None => break,
            }
        }
        if let Some(directory) = shallowest {
            found.insert(directory.join("Cargo.toml"));
        }
    }
    found.into_iter().collect()
}
