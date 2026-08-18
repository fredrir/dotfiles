//! Sizes and line counts for files and directories.
//!
//! `size` on its own lists the current directory. A directory target prints
//! its total, `-r` lists a directory's immediate contents, `-R` recurses
//! (`-L` limits how deep the listing goes), and `-l` swaps bytes for line
//! counts everywhere. Totals always include hidden files; `-a` only decides
//! whether hidden entries get their own rows. `-i` is the other way round: a
//! matching entry leaves the walk entirely, taking its bytes and — if it is a
//! directory — everything underneath it. A file reachable under more than one
//! name inside the tree counts once, the way `du` counts it. Bytes are the
//! space the tree occupies, blocks and all, so a sparse disk image measures
//! what it holds rather than what it claims and a directory of tiny files
//! measures the blocks they round up to; `-A` asks for the logical lengths
//! instead. Listings run smallest to biggest, so the largest entries sit next
//! to the total. On a terminal, names get the same colors and Nerd Font icons
//! eza-flavoured `ls` shows.

use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[cfg(target_vendor = "apple")]
mod bulk;

use clap::{Parser, ValueHint};
use rayon::prelude::*;
use workstation::Completions;

const PROGRAM: &str = "size";

/// Directory metadata lookups contend inside the kernel: on APFS, concurrent
/// stats in one directory serialise, and the contention grows with the thread
/// count rather than the work. Measured on a 285k-entry tree, wall clock
/// bottoms out near four threads while total CPU keeps climbing — a pool one
/// thread per core wide is both slower and five times dearer than this.
const WALK_THREADS: usize = 4;

#[derive(Parser)]
#[command(version, about = "Sizes and line counts for files and directories")]
struct Cli {
    /// File or directory to measure (no target: list the current directory)
    #[arg(value_hint = ValueHint::AnyPath)]
    target: Option<PathBuf>,

    /// List the immediate contents of the directory
    #[arg(short = 'r')]
    list: bool,

    /// List contents recursively
    #[arg(short = 'R')]
    recursive: bool,

    /// Count lines instead of bytes
    #[arg(short = 'l', long = "lines")]
    lines: bool,

    /// Measure logical lengths rather than the space actually taken up
    #[arg(short = 'A', long = "apparent", conflicts_with = "lines")]
    apparent: bool,

    /// Limit how deep the recursive listing goes
    #[arg(
        short = 'L',
        long = "limit",
        value_name = "DEPTH",
        requires = "recursive"
    )]
    limit: Option<usize>,

    /// Include hidden entries in listings (totals always include them)
    #[arg(short = 'a', long = "all")]
    all: bool,

    /// Leave matching entries out of the listing and the totals (repeatable)
    #[arg(short = 'i', long = "ignore", value_name = "PATTERN")]
    ignore: Vec<String>,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Default, Clone, Copy)]
struct Measure {
    bytes: u64,
    lines: u64,
    unreadable: usize,
}

impl Measure {
    fn add(&mut self, other: Measure) {
        self.bytes += other.bytes;
        self.lines += other.lines;
        self.unreadable += other.unreadable;
    }
}

struct Row {
    name: String,
    kind: &'static str,
    executable: bool,
    measure: Measure,
}

#[derive(Default)]
struct Options {
    lines: bool,
    all: bool,
    apparent: bool,
    display_depth: usize,
    ignore: Ignore,
}

/// What a walk found: the totals, the rows worth showing, and every file that
/// turned out to have more than one name. The links ride along with the rest
/// rather than through a shared collection, because a tree of build output can
/// be almost entirely hardlinks and a lock per file is a lock too many.
#[derive(Default)]
struct Walked {
    measure: Measure,
    rows: Vec<Row>,
    links: Vec<Link>,
}

impl Walked {
    fn of(measure: Measure) -> Walked {
        Walked {
            measure,
            ..Walked::default()
        }
    }

    fn unreadable() -> Walked {
        Walked::of(Measure {
            unreadable: 1,
            ..Measure::default()
        })
    }

    fn absorb(&mut self, other: Walked) {
        self.measure.add(other.measure);
        self.rows.extend(other.rows);
        self.links.extend(other.links);
    }
}

/// One name of a file that has several.
struct Link {
    file: (u64, u64),
    path: String,
    bytes: u64,
    lines: u64,
}

/// Patterns whose matches never enter the walk: no row, no bytes, and for a
/// directory, nothing underneath it either.
///
/// A pattern holding a `/` is matched against the path relative to the target,
/// anything else against the entry's own name. `*` stands for any run of
/// characters, `?` for exactly one. A trailing slash is dropped, so `-i bin/`
/// and `-i bin` mean the same thing — completion tends to supply the slash.
#[derive(Default)]
struct Ignore {
    names: Vec<String>,
    paths: Vec<String>,
}

impl Ignore {
    fn new(patterns: &[String]) -> Ignore {
        let mut ignore = Ignore::default();
        for pattern in patterns {
            let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
            if pattern.is_empty() {
                continue;
            }
            if pattern.contains('/') {
                ignore.paths.push(pattern.to_string());
            } else {
                ignore.names.push(pattern.to_string());
            }
        }
        ignore
    }

    fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }

    fn wants_paths(&self) -> bool {
        !self.paths.is_empty()
    }

    fn skips_name(&self, name: &str) -> bool {
        self.names.iter().any(|pattern| matches(pattern, name))
    }

    fn skips_path(&self, path: &str) -> bool {
        self.paths.iter().any(|pattern| matches(pattern, path))
    }
}

/// Glob matching: `*` for any run of characters, `?` for one. Backtracking is
/// bounded — a `*` only ever gives back a single character at a time.
fn matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut at_pattern, mut at_text) = (0, 0);
    let (mut star, mut resume) = (None, 0);
    while at_text < text.len() {
        if at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == text[at_text])
        {
            at_pattern += 1;
            at_text += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some(at_pattern);
            resume = at_text;
            at_pattern += 1;
        } else if let Some(star) = star {
            at_pattern = star + 1;
            resume += 1;
            at_text = resume;
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|glyph| *glyph == '*')
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    let named_target = cli.target.is_some();
    let target = cli.target.unwrap_or_else(|| PathBuf::from("."));
    let target = match resolve(target) {
        Ok(path) => path,
        Err(message) => return workstation::fail(PROGRAM, message),
    };

    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) => {
            return workstation::fail(PROGRAM, format!("{}: {error}", target.display()));
        }
    };

    let listing = cli.list || cli.recursive || !named_target;
    let display_depth = if cli.recursive {
        cli.limit.unwrap_or(usize::MAX).max(1)
    } else {
        1
    };
    let options = Options {
        lines: cli.lines,
        all: cli.all,
        apparent: cli.apparent,
        display_depth,
        ignore: Ignore::new(&cli.ignore),
    };

    if !metadata.is_dir() {
        if cli.list || cli.recursive {
            return workstation::fail(PROGRAM, format!("not a directory: {}", target.display()));
        }
        let measure = measure_file(&target, &metadata, &options);
        println!("{}", plain_value(measure, cli.lines));
        return done(measure.unreadable);
    }

    let Walked {
        measure: mut total,
        mut rows,
        links,
    } = walk(&options, &target);
    // The walks answer for a directory's contents; its own blocks are the
    // caller's to add, so that a subdirectory is counted by whoever lists it
    // and the target itself exactly once, here.
    total.bytes += directory_bytes(&metadata, &options);
    dedupe(&mut total, &mut rows, links);

    if !listing {
        println!("{}", plain_value(total, cli.lines));
        return done(total.unreadable);
    }

    sort_rows(&mut rows, cli.lines);
    print_table(&rows, total, cli.lines);
    done(total.unreadable)
}

/// The walk: in bulk where the platform allows it, otherwise a pool sized for
/// contention rather than for cores.
fn walk(options: &Options, target: &Path) -> Walked {
    let walk = || {
        #[cfg(target_vendor = "apple")]
        if let Some(walked) = bulk::walk(options, target) {
            return walked;
        }
        walk_directory(options, target, Path::new(""), 0)
    };
    let threads = std::thread::available_parallelism()
        .map_or(WALK_THREADS, |cores| cores.get().min(WALK_THREADS));
    match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
        Ok(pool) => pool.install(walk),
        Err(_) => walk(),
    }
}

/// A file reachable under several names is still one file, so count it for the
/// first of those names and not again for the rest — what `du` does with
/// hardlinks. "First" is the lowest path rather than whichever name the walk
/// happened to reach first, so a parallel walk still answers the same thing
/// twice running. Every directory the repeat sat under loses those bytes too,
/// which keeps each row agreeing with the total below it.
fn dedupe(total: &mut Measure, rows: &mut [Row], mut links: Vec<Link>) {
    if links.is_empty() {
        return;
    }
    links.sort_unstable_by(|a, b| a.file.cmp(&b.file).then(a.path.cmp(&b.path)));

    // Every name after the first for a given file. Most trees have none, and
    // one that does still only pays for the repeats themselves.
    let repeats: Vec<&Link> = links
        .windows(2)
        .filter(|pair| pair[0].file == pair[1].file)
        .map(|pair| &pair[1])
        .collect();
    if repeats.is_empty() {
        return;
    }
    for repeat in &repeats {
        total.bytes -= repeat.bytes;
        total.lines -= repeat.lines;
    }
    if rows.is_empty() {
        return;
    }

    let mut adjustments: Vec<(usize, u64, u64)> = Vec::new();
    {
        let index: HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .map(|(at, row)| (row.name.as_str(), at))
            .collect();
        for repeat in repeats {
            let ancestors = std::iter::successors(Some(repeat.path.as_str()), |path| {
                path.rsplit_once('/').map(|(parent, _)| parent)
            });
            for ancestor in ancestors {
                if let Some(&at) = index.get(ancestor) {
                    adjustments.push((at, repeat.bytes, repeat.lines));
                }
            }
        }
    }
    for (at, bytes, lines) in adjustments {
        rows[at].measure.bytes -= bytes;
        rows[at].measure.lines -= lines;
    }
}

fn done(unreadable: usize) -> ExitCode {
    if unreadable > 0 {
        let plural = if unreadable == 1 { "entry" } else { "entries" };
        eprintln!("{PROGRAM}: {unreadable} {plural} could not be read");
    }
    ExitCode::SUCCESS
}

/// A target like `document_1` resolves to `document_1.txt` when exactly one
/// directory entry starts with the given name.
fn resolve(target: PathBuf) -> Result<PathBuf, String> {
    if fs::symlink_metadata(&target).is_ok() {
        return Ok(target);
    }
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let stem = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut matches: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&stem) && !stem.is_empty() {
                if parent == Path::new(".") {
                    matches.push(PathBuf::from(name));
                } else {
                    matches.push(parent.join(name));
                }
            }
        }
    }
    match matches.as_slice() {
        [only] => {
            eprintln!("{PROGRAM}: {} -> {}", target.display(), only.display());
            Ok(only.clone())
        }
        [] => Err(format!("no such file or directory: {}", target.display())),
        _ => Err(format!(
            "no such file or directory: {} ({} entries match that prefix)",
            target.display(),
            matches.len()
        )),
    }
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    !metadata.is_dir() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn nlink(metadata: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(metadata)
}

/// What makes a file the same file under any of its names.
#[cfg(unix)]
fn file_id(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn nlink(_metadata: &fs::Metadata) -> u64 {
    1
}

#[cfg(not(unix))]
fn file_id(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn kind_of(metadata: &fs::Metadata) -> &'static str {
    if metadata.is_dir() {
        "directory"
    } else if metadata.is_symlink() {
        "link"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
}

/// What a file costs the filesystem, which is what `du` reports and what
/// deleting it hands back. It is not the length: a sparse file — a disk image,
/// a database, anything under `/proc` — claims a length it has never written,
/// and a tree of small files costs the block each one rounds up to. Both gaps
/// run to orders of magnitude, in opposite directions.
#[cfg(unix)]
fn allocated(metadata: &fs::Metadata) -> u64 {
    // POSIX fixes `st_blocks` at 512 bytes a block, whatever the filesystem
    // allocates in.
    std::os::unix::fs::MetadataExt::blocks(metadata) * 512
}

#[cfg(not(unix))]
fn allocated(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// A directory's own entry blocks, which `du` counts and a tree of many small
/// directories pays for. In apparent mode there is nothing to count: the
/// length a directory reports is a filesystem's bookkeeping rather than
/// anything the tree holds.
fn directory_bytes(metadata: &fs::Metadata, options: &Options) -> u64 {
    if options.apparent {
        0
    } else {
        allocated(metadata)
    }
}

fn measure_file(path: &Path, metadata: &fs::Metadata, options: &Options) -> Measure {
    let mut measure = Measure {
        bytes: if options.apparent {
            metadata.len()
        } else {
            allocated(metadata)
        },
        ..Measure::default()
    };
    if options.lines && metadata.is_file() {
        match count_lines(path) {
            Some(lines) => measure.lines = lines,
            None => measure.unreadable += 1,
        }
    }
    measure
}

thread_local! {
    /// One scratch buffer per worker: a fresh 256 KiB allocation per file was
    /// costing more than the reads it served.
    static BUFFER: std::cell::RefCell<Vec<u8>> =
        std::cell::RefCell::new(vec![0u8; 256 * 1024]);
}

fn count_lines(path: &Path) -> Option<u64> {
    count_lines_in(&mut fs::File::open(path).ok()?)
}

fn count_lines_in(file: &mut fs::File) -> Option<u64> {
    BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        let mut lines = 0u64;
        let mut first = true;
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                return Some(lines);
            }
            // A NUL early in the file marks it binary; newlines in binary data
            // are noise, not lines.
            if first && memchr::memchr(0, &buffer[..read]).is_some() {
                return Some(0);
            }
            first = false;
            lines += memchr::memchr_iter(b'\n', &buffer[..read]).count() as u64;
        }
    })
}

/// Depth-first aggregate of everything under `directory`, hidden included,
/// with every directory's entries processed in parallel. Rows are only
/// recorded down to the display depth and, unless `-a`, only for visible
/// entries — an invisible directory hides its whole subtree from the listing
/// while still counting toward every total.
fn walk_directory(options: &Options, directory: &Path, relative: &Path, depth: usize) -> Walked {
    let entries: Vec<fs::DirEntry> = match fs::read_dir(directory) {
        Ok(entries) => entries.flatten().collect(),
        Err(_) => return Walked::unreadable(),
    };
    entries
        .into_par_iter()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !options.ignore.is_empty() && options.ignore.skips_name(&name) {
                return Walked::default();
            }
            let child_relative = relative.join(&name);
            if options.ignore.wants_paths()
                && options.ignore.skips_path(&child_relative.to_string_lossy())
            {
                return Walked::default();
            }
            let Ok(metadata) = entry.metadata() else {
                return Walked::unreadable();
            };
            let visible = depth < options.display_depth && (options.all || !hidden(&name));
            let mut walked = if metadata.is_dir() {
                // A hidden directory's children stay out of the listing even
                // with room left in the depth budget, so cap their display
                // depth.
                let child_depth = if visible {
                    depth + 1
                } else {
                    options.display_depth
                };
                let mut walked =
                    walk_directory(options, &entry.path(), &child_relative, child_depth);
                walked.measure.bytes += directory_bytes(&metadata, options);
                walked
            } else {
                let measure = measure_file(&entry.path(), &metadata, options);
                let mut walked = Walked::of(measure);
                if nlink(&metadata) > 1 {
                    walked.links.push(Link {
                        file: file_id(&metadata),
                        path: child_relative.to_string_lossy().to_string(),
                        bytes: measure.bytes,
                        lines: measure.lines,
                    });
                }
                walked
            };
            if visible {
                let measure = walked.measure;
                walked.rows.push(Row {
                    name: child_relative.to_string_lossy().to_string(),
                    kind: kind_of(&metadata),
                    executable: is_executable(&metadata),
                    measure,
                });
            }
            walked
        })
        .reduce(Walked::default, |mut left, right| {
            left.absorb(right);
            left
        })
}

/// Smallest first, biggest last — the largest entries land next to the total,
/// where the eye already is when the listing scrolls.
fn sort_rows(rows: &mut [Row], lines: bool) {
    rows.sort_unstable_by(|a, b| {
        let metric = |row: &Row| {
            if lines {
                row.measure.lines
            } else {
                row.measure.bytes
            }
        };
        metric(a).cmp(&metric(b)).then(a.name.cmp(&b.name))
    });
}

fn plain_value(measure: Measure, lines: bool) -> String {
    if lines {
        grouped(measure.lines)
    } else {
        human_size(measure.bytes)
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in UNITS {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The colors ls would use, resolved once for the whole table rather than
/// re-parsed per row.
struct Palette {
    directory: String,
    link: String,
    executable: String,
    file: Option<String>,
    extensions: HashMap<String, String>,
}

impl Palette {
    /// The theme unsets LS_COLORS and ships the same entries as EZA_COLORS, so
    /// read that first and fall back to LS_COLORS, then to the GNU defaults.
    fn from_env() -> Palette {
        let table = std::env::var("EZA_COLORS")
            .ok()
            .filter(|table| !table.is_empty())
            .or_else(|| std::env::var("LS_COLORS").ok())
            .unwrap_or_default();
        Palette::parse(&table)
    }

    /// Later entries win, the way both `ls` and eza read these tables: the
    /// theme states a category and then overrides single extensions after it.
    fn parse(table: &str) -> Palette {
        let mut palette = Palette {
            directory: "01;34".to_string(),
            link: "01;36".to_string(),
            executable: "01;32".to_string(),
            file: None,
            extensions: HashMap::new(),
        };
        for entry in table.split(':') {
            let Some((key, color)) = entry.split_once('=') else {
                continue;
            };
            match key {
                "di" => palette.directory = color.to_string(),
                "ln" => palette.link = color.to_string(),
                "ex" => palette.executable = color.to_string(),
                "fi" => palette.file = Some(color.to_string()),
                _ => {
                    if let Some(extension) = key.strip_prefix("*.") {
                        palette
                            .extensions
                            .insert(extension.to_lowercase(), color.to_string());
                    }
                }
            }
        }
        palette
    }

    /// Kind first, the way `ls` orders it: a directory, link or executable is
    /// coloured for what it is, and only a plain file is coloured for what it
    /// is named.
    fn color(&self, row: &Row, base: &str) -> Option<&str> {
        match row.kind {
            "directory" => Some(&self.directory),
            "link" => Some(&self.link),
            _ if row.executable => Some(&self.executable),
            _ => extension_of(base)
                .and_then(|extension| self.extensions.get(extension))
                .map(String::as_str)
                .or(self.file.as_deref()),
        }
    }
}

/// The final path component, lowercased: what the colour table and the icon
/// table both match against.
fn basename(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_lowercase()
}

fn extension_of(base: &str) -> Option<&str> {
    match base.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => Some(extension),
        _ => None,
    }
}

/// The Nerd Font glyph eza would show for this entry.
fn icon_for(row: &Row, base: &str) -> char {
    if row.kind == "directory" {
        return '\u{f115}';
    }
    match base {
        "dockerfile" => return '\u{e650}',
        "makefile" | "justfile" => return '\u{e673}',
        "license" | "license.md" | "license.txt" => return '\u{f02d}',
        "readme" | "readme.md" => return '\u{f00ba}',
        _ => {}
    }
    if base.starts_with(".git") {
        return '\u{f02a2}';
    }
    match extension_of(base).unwrap_or("") {
        "rs" => '\u{e68b}',
        "py" => '\u{e606}',
        "js" | "mjs" | "cjs" => '\u{e74e}',
        "ts" => '\u{e628}',
        "tsx" | "jsx" => '\u{e7ba}',
        "json" => '\u{e60b}',
        "toml" => '\u{e6b2}',
        "yaml" | "yml" => '\u{e8eb}',
        "md" => '\u{f48a}',
        "sh" | "zsh" | "bash" => '\u{f489}',
        "lua" => '\u{e620}',
        "go" => '\u{e65e}',
        "c" | "h" => '\u{e61e}',
        "cpp" | "cc" | "hpp" => '\u{e61d}',
        "html" => '\u{f13b}',
        "css" => '\u{e749}',
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => '\u{f1c5}',
        "svg" => '\u{f0559}',
        "pdf" => '\u{f1c1}',
        "zip" | "gz" | "tar" | "xz" | "zst" | "bz2" | "7z" => '\u{f410}',
        "lock" => '\u{f023}',
        "conf" | "cfg" | "ini" => '\u{f107b}',
        "nix" => '\u{f313}',
        "txt" => '\u{f15c}',
        _ => '\u{f086f}',
    }
}

fn print_table(rows: &[Row], total: Measure, lines: bool) {
    let stdout = std::io::stdout();
    let styled = stdout.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let (dim, bold, reset) = if styled {
        ("\x1b[2m", "\x1b[1m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    // Icons occupy two cells (glyph + space) in front of every name.
    let prefix = if styled { 2 } else { 0 };
    let value_header = if lines { "LINES" } else { "SIZE" };
    let name_width = rows
        .iter()
        .map(|row| row.name.chars().count() + prefix)
        .chain(["NAME".len(), "Total".len()])
        .max()
        .unwrap_or(4);
    let total_text = plain_value(total, lines);
    let values: Vec<String> = rows
        .iter()
        .map(|row| plain_value(row.measure, lines))
        .collect();
    let value_width = values
        .iter()
        .map(|value| value.chars().count())
        .chain([value_header.len(), total_text.chars().count()])
        .max()
        .unwrap_or(4);

    // One buffer for the whole table: a listing is thousands of rows, and
    // stdout would otherwise flush on every one of them.
    let palette = styled.then(Palette::from_env);
    let mut out = BufWriter::with_capacity(128 * 1024, stdout.lock());
    for (row, value) in rows.iter().zip(&values) {
        let padding = " ".repeat(name_width - row.name.chars().count() - prefix);
        let _ = match palette.as_ref() {
            Some(palette) => {
                let base = basename(&row.name);
                let icon = icon_for(row, &base);
                match palette.color(row, &base) {
                    Some(color) => writeln!(
                        out,
                        "\x1b[{color}m{icon} {}{reset}{padding}  {value:>value_width$}",
                        row.name
                    ),
                    None => writeln!(out, "{icon} {}{padding}  {value:>value_width$}", row.name),
                }
            }
            None => writeln!(out, "{}{padding}  {value:>value_width$}", row.name),
        };
    }
    let width = name_width + value_width + 2;
    let _ = writeln!(out, "{dim}{}{reset}", "─".repeat(width));
    let _ = writeln!(
        out,
        "{bold}{:<name_width$}  {:>value_width$}{reset}",
        "Total", total_text
    );
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("notes.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(root.path().join(".secret"), "hidden\n").unwrap();
        fs::create_dir(root.path().join("assets")).unwrap();
        fs::write(root.path().join("assets/a.bin"), vec![0u8; 2500]).unwrap();
        fs::create_dir(root.path().join(".cache")).unwrap();
        fs::write(root.path().join(".cache/blob"), vec![0u8; 100]).unwrap();
        root
    }

    /// Options that measure logical lengths, so a test can assert the bytes it
    /// wrote rather than whatever the filesystem rounded them up to. The
    /// default — space on disk — gets its own tests below.
    fn logical() -> Options {
        Options {
            apparent: true,
            ..Options::default()
        }
    }

    fn walk_all(root: &Path, lines: bool, all: bool, depth: usize) -> (Vec<Row>, Measure) {
        let options = Options {
            lines,
            all,
            display_depth: depth,
            ..logical()
        };
        let Walked {
            measure, mut rows, ..
        } = walk_directory(&options, root, Path::new(""), 0);
        sort_rows(&mut rows, lines);
        (rows, measure)
    }

    #[test]
    fn totals_include_hidden_files_rows_do_not() {
        let root = tree();
        let (rows, total) = walk_all(root.path(), false, false, 1);
        assert_eq!(total.bytes, 14 + 7 + 2500 + 100);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["notes.txt", "assets"]);
        let assets = rows.iter().find(|row| row.name == "assets").unwrap();
        assert_eq!(assets.measure.bytes, 2500);
    }

    /// A file that claims a length it never wrote costs the blocks it holds
    /// and nothing more — the disk image, the database, everything under
    /// `/proc`. Only `-A` should believe the claim.
    #[test]
    fn a_sparse_file_measures_what_it_holds() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.raw");
        let claimed = 64 * 1024 * 1024;
        fs::File::create(&path).unwrap().set_len(claimed).unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();

        assert_eq!(measure_file(&path, &metadata, &logical()).bytes, claimed);
        let on_disk = measure_file(&path, &metadata, &Options::default()).bytes;
        assert!(
            on_disk < claimed,
            "{on_disk} blocks for a hole of {claimed}"
        );
    }

    /// Disk mode has to account for every entry in the tree, directories
    /// included. The aggregation is the part that can drift, so add the same
    /// tree up a second way and compare.
    #[test]
    fn disk_mode_counts_every_entry_the_tree_holds() {
        fn by_hand(directory: &Path) -> u64 {
            fs::read_dir(directory)
                .unwrap()
                .flatten()
                .map(|entry| {
                    let metadata = entry.metadata().unwrap();
                    let below = if metadata.is_dir() {
                        by_hand(&entry.path())
                    } else {
                        0
                    };
                    allocated(&metadata) + below
                })
                .sum()
        }

        let root = tree();
        let options = Options {
            display_depth: usize::MAX,
            ..Options::default()
        };
        let walked = walk_directory(&options, root.path(), Path::new(""), 0);
        assert_eq!(walked.measure.bytes, by_hand(root.path()));
    }

    #[test]
    fn all_reveals_hidden_entries() {
        let root = tree();
        let (rows, _) = walk_all(root.path(), false, true, 1);
        assert!(rows.iter().any(|row| row.name == ".secret"));
        assert!(rows.iter().any(|row| row.name == ".cache"));
    }

    #[test]
    fn depth_limits_the_listing_not_the_totals() {
        let root = tree();
        let (shallow, total) = walk_all(root.path(), false, false, 1);
        assert!(!shallow.iter().any(|row| row.name.contains("a.bin")));
        let (deep, deep_total) = walk_all(root.path(), false, false, usize::MAX);
        assert!(
            deep.iter()
                .any(|row| row.name == Path::new("assets").join("a.bin").to_string_lossy())
        );
        assert!(!deep.iter().any(|row| row.name.contains("blob")));
        assert_eq!(total.bytes, deep_total.bytes);
    }

    #[test]
    fn line_mode_counts_newlines_recursively() {
        let root = tree();
        let (rows, total) = walk_all(root.path(), true, false, 1);
        assert_eq!(total.lines, 3 + 1);
        let notes = rows.iter().find(|row| row.name == "notes.txt").unwrap();
        assert_eq!(notes.measure.lines, 3);
    }

    #[test]
    fn binary_files_count_zero_lines() {
        let root = tree();
        let mut noise = vec![0u8; 64];
        noise.extend_from_slice(b"\n\n\n");
        fs::write(root.path().join("blob.bin"), noise).unwrap();
        let (rows, _) = walk_all(root.path(), true, false, 1);
        let blob = rows.iter().find(|row| row.name == "blob.bin").unwrap();
        assert_eq!(blob.measure.lines, 0);
    }

    #[test]
    fn line_mode_sorts_by_lines() {
        let root = tree();
        fs::write(root.path().join("tiny.txt"), "a\nb\nc\nd\ne\n").unwrap();
        let (rows, _) = walk_all(root.path(), true, false, 1);
        assert_eq!(rows.last().unwrap().name, "tiny.txt");
    }

    #[test]
    fn rows_sort_smallest_first_whatever_their_kind() {
        let root = tree();
        fs::write(root.path().join("big.txt"), vec![b'x'; 9000]).unwrap();
        let (rows, _) = walk_all(root.path(), false, false, 1);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["notes.txt", "assets", "big.txt"]);
    }

    #[test]
    fn human_sizes_match_the_house_style() {
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(3000), "3.0 kB");
        assert_eq!(human_size(1_400), "1.4 kB");
        assert_eq!(human_size(31_000_000), "31 MB");
        assert_eq!(human_size(45_300_000), "45 MB");
    }

    #[test]
    fn unique_prefix_resolves() {
        let root = tree();
        let target = root.path().join("note");
        assert_eq!(resolve(target).unwrap(), root.path().join("notes.txt"));
        assert!(resolve(root.path().join("nothing")).is_err());
    }

    #[test]
    fn grouped_thousands() {
        assert_eq!(grouped(58), "58");
        assert_eq!(grouped(12345), "12,345");
    }

    fn row(name: &str, kind: &'static str) -> Row {
        Row {
            name: name.to_string(),
            kind,
            executable: false,
            measure: Measure::default(),
        }
    }

    #[test]
    fn the_palette_reads_the_themes_entries() {
        let palette = Palette::parse("reset:fi=38;2;186:di=38;2;61:ex=38;2;124:ln=38;2;26");
        assert_eq!(
            palette.color(&row("src", "directory"), "src"),
            Some("38;2;61")
        );
        assert_eq!(palette.color(&row("link", "link"), "link"), Some("38;2;26"));
        assert_eq!(
            palette.color(&row("notes.txt", "file"), "notes.txt"),
            Some("38;2;186")
        );
        let mut runnable = row("build.sh", "file");
        runnable.executable = true;
        assert_eq!(palette.color(&runnable, "build.sh"), Some("38;2;124"));
    }

    #[test]
    fn an_empty_table_falls_back_to_the_gnu_defaults() {
        let palette = Palette::parse("");
        assert_eq!(
            palette.color(&row("src", "directory"), "src"),
            Some("01;34")
        );
        assert_eq!(palette.color(&row("notes.txt", "file"), "notes.txt"), None);
    }

    /// `di` must not match `dirty=`; only a whole key followed by `=` counts.
    #[test]
    fn a_longer_key_is_not_mistaken_for_a_shorter_one() {
        let palette = Palette::parse("dirty=1:link=2");
        assert_eq!(
            palette.color(&row("src", "directory"), "src"),
            Some("01;34")
        );
        assert_eq!(palette.color(&row("l", "link"), "l"), Some("01;36"));
    }

    /// A tree where one file is reachable under two names, plus a name that
    /// sorts before both so the walk has to pick rather than take the first.
    fn linked() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("apples")).unwrap();
        fs::create_dir(root.path().join("bananas")).unwrap();
        fs::write(root.path().join("apples/one.txt"), "hello\n").unwrap();
        fs::hard_link(
            root.path().join("apples/one.txt"),
            root.path().join("bananas/two.txt"),
        )
        .unwrap();
        fs::write(root.path().join("solo.txt"), "abc").unwrap();
        root
    }

    fn walk_and_dedupe(options: &Options, root: &Path) -> (Measure, Vec<Row>) {
        let Walked {
            measure: mut total,
            mut rows,
            links,
        } = walk_directory(options, root, Path::new(""), 0);
        dedupe(&mut total, &mut rows, links);
        (total, rows)
    }

    #[test]
    fn a_file_with_two_names_counts_once() {
        let root = linked();
        let options = Options {
            display_depth: usize::MAX,
            ..logical()
        };
        let (total, _) = walk_and_dedupe(&options, root.path());
        // 6 for the linked file, counted once, and 3 for the other.
        assert_eq!(total.bytes, 6 + 3);
    }

    /// The lowest path keeps the bytes, so a parallel walk cannot change which
    /// of the names is the one that counts.
    #[test]
    fn the_lowest_path_is_the_one_that_counts() {
        let root = linked();
        let options = Options {
            display_depth: usize::MAX,
            ..logical()
        };
        for _ in 0..8 {
            let (_, rows) = walk_and_dedupe(&options, root.path());
            let bytes = |name: &str| {
                rows.iter()
                    .find(|row| row.name == name)
                    .unwrap_or_else(|| panic!("no row for {name}"))
                    .measure
                    .bytes
            };
            assert_eq!(bytes("apples/one.txt"), 6);
            assert_eq!(bytes("bananas/two.txt"), 0);
            // The directory the repeat sat in loses the bytes with it, so the
            // rows still agree with the total.
            assert_eq!(bytes("apples"), 6);
            assert_eq!(bytes("bananas"), 0);
        }
    }

    /// Only one of the names is inside the tree, so there is nothing to
    /// discount — the same answer `du` gives.
    #[test]
    fn a_link_from_outside_the_tree_still_counts() {
        let root = linked();
        let options = Options {
            display_depth: usize::MAX,
            ..logical()
        };
        let (total, _) = walk_and_dedupe(&options, &root.path().join("apples"));
        assert_eq!(total.bytes, 6);
    }

    #[test]
    fn globs_match_the_way_the_shell_would() {
        assert!(matches("bin", "bin"));
        assert!(!matches("bin", "binary"));
        assert!(matches("*.log", "server.log"));
        assert!(!matches("*.log", "server.log.gz"));
        assert!(matches("*.log*", "server.log.gz"));
        assert!(matches("node_*", "node_modules"));
        assert!(matches("?ar", "tar"));
        assert!(!matches("?ar", "star"));
        assert!(matches("*", "anything"));
        assert!(matches("*", ""));
        assert!(matches("a*b*c", "axxbyyc"));
        assert!(!matches("a*b*c", "axxbyy"));
    }

    #[test]
    fn ignored_entries_leave_the_totals_as_well_as_the_rows() {
        let root = tree();
        let ignore = Ignore::new(&["assets".to_string()]);
        let options = Options {
            display_depth: 1,
            ignore,
            ..logical()
        };
        let walked = walk_directory(&options, root.path(), Path::new(""), 0);
        // assets/a.bin is 2500 of the tree's bytes, and goes with it.
        assert_eq!(walked.measure.bytes, 14 + 7 + 100);
        assert!(!walked.rows.iter().any(|row| row.name == "assets"));
    }

    #[test]
    fn a_trailing_slash_on_a_pattern_is_ignored() {
        let root = tree();
        for pattern in ["assets", "assets/"] {
            let options = Options {
                display_depth: 1,
                ignore: Ignore::new(&[pattern.to_string()]),
                ..logical()
            };
            let walked = walk_directory(&options, root.path(), Path::new(""), 0);
            assert_eq!(walked.measure.bytes, 14 + 7 + 100, "pattern {pattern}");
        }
    }

    /// A pattern with a slash in it is about where the entry sits, not what it
    /// is called.
    #[test]
    fn a_pattern_with_a_slash_matches_the_relative_path() {
        let root = tree();
        let by_path = Options {
            display_depth: usize::MAX,
            ignore: Ignore::new(&["assets/a.bin".to_string()]),
            ..logical()
        };
        let walked = walk_directory(&by_path, root.path(), Path::new(""), 0);
        assert_eq!(walked.measure.bytes, 14 + 7 + 100);

        // The same text as a bare name matches nothing, since no entry is
        // called "assets/a.bin".
        let by_name = Options {
            display_depth: usize::MAX,
            ignore: Ignore::new(&["a.bin".to_string()]),
            ..logical()
        };
        let walked = walk_directory(&by_name, root.path(), Path::new(""), 0);
        assert_eq!(walked.measure.bytes, 14 + 7 + 100);
    }

    #[test]
    fn extension_colours_come_from_the_table() {
        let palette = Palette::parse("fi=plain:*.rs=rust:*.MD=doc");
        assert_eq!(
            palette.color(&row("main.rs", "file"), "main.rs"),
            Some("rust")
        );
        // The table's case should not decide whether a file matches.
        assert_eq!(
            palette.color(&row("README.md", "file"), "readme.md"),
            Some("doc")
        );
        assert_eq!(
            palette.color(&row("notes.txt", "file"), "notes.txt"),
            Some("plain")
        );
    }

    /// What an entry *is* outranks what it is named, the way `ls` orders it.
    #[test]
    fn kind_outranks_extension() {
        let palette = Palette::parse("di=dir:ex=exec:ln=link:*.rs=rust");
        assert_eq!(
            palette.color(&row("src.rs", "directory"), "src.rs"),
            Some("dir")
        );
        assert_eq!(palette.color(&row("to.rs", "link"), "to.rs"), Some("link"));
        let mut runnable = row("build.rs", "file");
        runnable.executable = true;
        assert_eq!(palette.color(&runnable, "build.rs"), Some("exec"));
    }

    /// The theme names a category and then overrides single extensions after
    /// it, so the later entry has to be the one that sticks.
    #[test]
    fn a_later_entry_overrides_an_earlier_one() {
        let palette = Palette::parse("*.toml=first:*.toml=second:fi=one:fi=two");
        assert_eq!(
            palette.color(&row("Cargo.toml", "file"), "cargo.toml"),
            Some("second")
        );
        assert_eq!(palette.color(&row("plain", "file"), "plain"), Some("two"));
    }

    #[test]
    fn icons_follow_the_eza_table() {
        assert_eq!(
            icon_for(&row("src", "directory"), &basename("src")),
            '\u{f115}'
        );
        assert_eq!(
            icon_for(&row("main.rs", "file"), &basename("main.rs")),
            '\u{e68b}'
        );
        assert_eq!(
            icon_for(
                &row("deep/path/notes.md", "file"),
                &basename("deep/path/notes.md")
            ),
            '\u{f48a}'
        );
        assert_eq!(
            icon_for(&row("README.md", "file"), &basename("README.md")),
            '\u{f00ba}'
        );
        assert_eq!(
            icon_for(&row(".gitignore", "file"), &basename(".gitignore")),
            '\u{f02a2}'
        );
        assert_eq!(
            icon_for(&row("mystery", "file"), &basename("mystery")),
            '\u{f086f}'
        );
    }
}
