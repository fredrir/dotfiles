//! Handing the files to the tools that own them.
//!
//! Languages run in parallel and the steps within one language run in order.
//! That split is not an optimisation, it is a correctness rule: `goimports`
//! and `gofmt` both rewrite the same `.go` file, so letting the two `-w`
//! passes overlap is a race on the file itself. Two different languages can
//! never be handed the same path, so there is nothing to race between them.

use std::collections::{BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use rayon::prelude::*;

use crate::configs::{self, Injected, Scope};
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
    /// The files a tool pointed at in what it said, so a failure names a file
    /// without the whole of the tool's output having to be shown. Filled in
    /// only when there is something to report and only when the output is not
    /// being shown anyway.
    pub blamed: Vec<String>,
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
            blamed: Vec::new(),
        }
    }

    /// A row for a provider that is installed and could not answer, over a
    /// tree it would have had nothing to do in anyway.
    pub fn broken(lang: Lang, said: String) -> Ran {
        Ran {
            failed: true,
            output: said,
            ..Ran::new(lang, 0)
        }
    }

    /// Record that dotfmt could not be asked which files it owns.
    ///
    /// The row still ran, over the files this crate guessed at, so what is
    /// added is the reason the guess was needed. It replaces the shortlist
    /// rather than joining it: a run whose selection rule could not be read
    /// needs to say so before it says which files came of it, and the files
    /// are one `--verbose` away.
    pub fn unasked(&mut self, said: &str) {
        self.failed = true;
        self.output.insert_str(0, said);
        self.blamed.clear();
    }
}

/// What a run is, apart from the files it is over.
pub struct Plan<'a> {
    pub mode: Mode,
    pub verbose: bool,
    /// Extra arguments for the programs that have to be pointed at this
    /// repository's config, because they would not find it themselves.
    pub injected: &'a [Injected],
}

impl Plan<'_> {
    /// A step's files, split into the sets that take different arguments.
    ///
    /// One group for a program with nothing to inject, one for a program that
    /// resolves from its working directory, and two for a program that
    /// resolves per file — because naming a config on the command line
    /// outranks a nearer one, so the files that have their own have to be run
    /// without it.
    fn groups(&self, root: &Path, program: &str, files: &[PathBuf]) -> Vec<Grouped> {
        let Some(injected) = self
            .injected
            .iter()
            .find(|injected| injected.program == program)
        else {
            return vec![(Vec::new(), files.to_vec())];
        };
        match injected.scope {
            Scope::Row => vec![(injected.args.clone(), files.to_vec())],
            Scope::Unclaimed(own) => {
                let (theirs, ours) = configs::partition(root, own, files);
                let mut groups = Vec::new();
                if !ours.is_empty() {
                    groups.push((injected.args.clone(), ours));
                }
                if !theirs.is_empty() {
                    groups.push((Vec::new(), theirs));
                }
                groups
            }
        }
    }
}

/// The arguments one set of files is run with, and the files.
type Grouped = (Vec<OsString>, Vec<PathBuf>);

/// Sort the candidates into their rows, dropping the rows with nothing in
/// them.
///
/// dotfmt's row is not built here. Which files it owns depends on
/// `include`/`exclude` patterns resolved per directory, so the extension list
/// in the table is a description of dotfmt rather than a decision about it;
/// `owns::ask` gets the real answer and `with_dotfmt` splices it in.
pub fn sort(files: Vec<PathBuf>) -> Vec<(Lang, Vec<PathBuf>)> {
    let mut work: Vec<(Lang, Vec<PathBuf>)> =
        LANGS.into_iter().map(|lang| (lang, Vec::new())).collect();
    for file in files {
        match Lang::of(&file) {
            Some(Lang::Dotfmt) | None => continue,
            Some(lang) => {
                let slot = work
                    .iter_mut()
                    .find(|(row, _)| *row == lang)
                    .expect("every language has a row");
                slot.1.push(file);
            }
        }
    }
    work.retain(|(_, files)| !files.is_empty());
    work
}

/// The files this crate would guess dotfmt owns, for the two cases where it
/// cannot be asked.
///
/// A guess is worse than an answer — it is the whole reason `--owns` exists —
/// but it is far better than an empty row, which would quietly stop formatting
/// every `.conf` in the tree and look like a clean run.
pub fn by_extension(files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|file| Lang::of(file) == Some(Lang::Dotfmt))
        .cloned()
        .collect()
}

/// Put dotfmt's answer back at the front of the table, where its row belongs.
pub fn with_dotfmt(
    mut work: Vec<(Lang, Vec<PathBuf>)>,
    owned: Vec<PathBuf>,
) -> Vec<(Lang, Vec<PathBuf>)> {
    if !owned.is_empty() {
        work.insert(0, (Lang::Dotfmt, owned));
    }
    work
}

/// `root` must be absolute: it is both the working directory every child is
/// given and the base the relative paths are resolved against.
pub fn run(root: &Path, work: Vec<(Lang, Vec<PathBuf>)>, plan: &Plan) -> Vec<Ran> {
    let mut done: Vec<Ran> = work
        .into_par_iter()
        .map(|(lang, files)| one(root, lang, &files, plan))
        .collect();
    order(&mut done);
    done
}

/// Back into table order, so two runs over the same tree read the same
/// however the threads finished.
pub fn order(done: &mut [Ran]) {
    done.sort_by_key(|ran| LANGS.iter().position(|lang| *lang == ran.lang));
}

fn one(root: &Path, lang: Lang, files: &[PathBuf], plan: &Plan) -> Ran {
    let mode = plan.mode;
    let mut ran = Ran::new(lang, files.len());
    for step in lang.steps(mode) {
        // `cargo fmt` takes a manifest rather than a file list, so it is asked
        // a different question of the same files and takes no injected
        // arguments — its config goes in a place this cannot reach.
        let groups: Vec<(Vec<OsString>, Vec<Vec<OsString>>)> = match step.feed {
            Feed::Manifests => vec![(Vec::new(), manifest_arguments(root, files))],
            Feed::Files => plan
                .groups(root, step.program, files)
                .into_iter()
                .map(|(extra, group)| (extra, batches(&group)))
                .collect(),
        };
        // Rust files with no manifest over them are nothing cargo can be
        // asked about, which is worth saying rather than passing over.
        if step.feed == Feed::Manifests && groups.iter().all(|(_, lines)| lines.is_empty()) {
            ran.note = Some("no Cargo.toml above these files".to_string());
            continue;
        }
        'step: for (extra, batches) in groups {
            for batch in batches {
                match invoke(root, &step, &extra, &batch) {
                    Ok(output) => {
                        ran.ran += 1;
                        absorb(
                            &mut ran,
                            &step,
                            &batch,
                            &output,
                            mode,
                            Told {
                                verbose: plan.verbose,
                                extra: &extra,
                            },
                        );
                    }
                    // A tool nobody installed is a fact about this machine,
                    // not a failure of this run, and the rest of the languages
                    // still have their tools.
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        ran.missing.push(step.program);
                        break 'step;
                    }
                    Err(error) => {
                        ran.failed = true;
                        ran.output.push_str(&format!("{}: {error}\n", step.program));
                        break 'step;
                    }
                }
            }
        }
    }
    // Under `--verbose` the whole of what the tools said is shown, so there is
    // nothing for a shortlist to add — and the command lines echoed there name
    // every path, which would make the shortlist the file list.
    if !plan.verbose && (ran.failed || ran.findings) {
        ran.blamed = blamed(&ran.output, root, files);
    }
    ran
}

/// Which of the files handed to a row were named in what its tools said.
///
/// Every one of them writes a path the way it was given the path, so no
/// per-tool parsing is needed: recognising the run's own paths is enough, and
/// a tool this crate has never heard of is read the same way as the ten it
/// has. `path:12:3:`, `path:` and taplo's `path="…"` are the decorations
/// worth undoing, and the root is worth undoing too — `cargo fmt` is handed a
/// manifest rather than a file list and answers in absolute paths.
pub fn blamed(output: &str, root: &Path, files: &[PathBuf]) -> Vec<String> {
    let known: HashSet<&OsStr> = files.iter().map(|path| path.as_os_str()).collect();
    let inside = format!("{}/", root.display());
    let mut found: Vec<String> = Vec::new();
    for word in output.split_whitespace() {
        let mut word = word.trim_matches(|letter: char| "`'\"(),[]{}<>".contains(letter));
        loop {
            let relative = word.strip_prefix(&inside).unwrap_or(word);
            if known.contains(OsStr::new(relative)) {
                let name = relative.to_string();
                if !found.contains(&name) {
                    found.push(name);
                }
                break;
            }
            if let Some(rest) = word.strip_suffix(':') {
                word = rest;
                continue;
            }
            // taplo writes `path="…"`, and the quote has already been
            // trimmed off the end of it.
            if let Some((_, rest)) = word.rsplit_once("=\"") {
                word = rest;
                continue;
            }
            match word.rsplit_once(':') {
                Some((head, tail))
                    if !tail.is_empty() && tail.chars().all(|digit| digit.is_ascii_digit()) =>
                {
                    word = head;
                }
                _ => break,
            }
        }
    }
    found
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

/// Every child runs in the root with relative paths, so stylua finds
/// `.stylua.toml` by the upward search it already does — and so that the two
/// tools whose config this run names outright are given a path relative to the
/// same directory. `stdin` is closed because a tool that decided to read from
/// the terminal would hang a run that has no business being interactive.
pub fn invoke(
    root: &Path,
    step: &Step,
    extra: &[OsString],
    trailing: &[OsString],
) -> io::Result<Output> {
    Command::new(step.program)
        .args(step.args)
        .args(extra)
        .args(trailing)
        .envs(step.env.iter().copied())
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
}

/// Whether to write down what was run, and what was added to it.
#[derive(Clone, Copy)]
struct Told<'a> {
    verbose: bool,
    /// The arguments this run added to the table's own, which are part of
    /// what ran and so part of what `--verbose` has to show.
    extra: &'a [OsString],
}

fn absorb(ran: &mut Ran, step: &Step, batch: &[OsString], output: &Output, mode: Mode, told: Told) {
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
    // exactly what ran is what the flag is for. Otherwise nothing is echoed at
    // all: a check over 194 files would open with 194 paths on one line, and
    // the report names the provider and the files it fell over on without
    // either. The command that produced them is one flag away.
    if told.verbose {
        ran.output.push_str(&full(step, told.extra, batch));
    }
    ran.output.push_str(&said(output));
}

/// The command exactly as it was run, argument for argument.
fn full(step: &Step, extra: &[OsString], batch: &[OsString]) -> String {
    format!(
        "$ {} {}\n",
        step.program,
        words(step, extra)
            .into_iter()
            .chain(batch.iter().map(|path| path.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn words(step: &Step, extra: &[OsString]) -> Vec<String> {
    step.args
        .iter()
        .map(|argument| (*argument).to_string())
        .chain(
            extra
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        )
        .collect()
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
