//! Sizes and line counts for files and directories.
//!
//! `size` on its own lists the current directory. A directory target prints
//! its total, `-r` lists a directory's immediate contents, `-R` recurses
//! (`-L` limits how deep the listing goes), and `-l` swaps bytes for line
//! counts everywhere. Totals always include hidden files; `-a` only decides
//! whether hidden entries get their own rows. On a terminal, names get the
//! same colors and Nerd Font icons eza-flavoured `ls` shows.

use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, ValueHint};
use rayon::prelude::*;

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

    /// Print shell completions and exit
    #[arg(long = "completions", value_name = "SHELL", exclusive = true)]
    completions: Option<clap_complete::Shell>,
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

struct Options {
    lines: bool,
    all: bool,
    display_depth: usize,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "size", &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }
    let named_target = cli.target.is_some();
    let target = cli.target.unwrap_or_else(|| PathBuf::from("."));
    let target = match resolve(target) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("size: {message}");
            return ExitCode::FAILURE;
        }
    };

    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("size: {}: {error}", target.display());
            return ExitCode::FAILURE;
        }
    };

    if !metadata.is_dir() {
        if cli.list || cli.recursive {
            eprintln!("size: not a directory: {}", target.display());
            return ExitCode::FAILURE;
        }
        let measure = measure_file(&target, &metadata, cli.lines);
        println!("{}", plain_value(measure, cli.lines));
        return done(measure.unreadable);
    }

    let listing = cli.list || cli.recursive || !named_target;
    let display_depth = if cli.recursive {
        cli.limit.unwrap_or(usize::MAX).max(1)
    } else {
        1
    };
    let options = Options {
        lines: cli.lines,
        all: cli.all,
        display_depth,
    };
    let (total, mut rows) = walk_directory(&options, &target, Path::new(""), 0);

    if !listing {
        println!("{}", plain_value(total, cli.lines));
        return done(total.unreadable);
    }

    sort_rows(&mut rows, cli.lines);
    print_table(&rows, total, cli.lines);
    done(total.unreadable)
}

fn done(unreadable: usize) -> ExitCode {
    if unreadable > 0 {
        let plural = if unreadable == 1 { "entry" } else { "entries" };
        eprintln!("size: {unreadable} {plural} could not be read");
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
            eprintln!("size: {} -> {}", target.display(), only.display());
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

fn measure_file(path: &Path, metadata: &fs::Metadata, want_lines: bool) -> Measure {
    let mut measure = Measure {
        bytes: metadata.len(),
        ..Measure::default()
    };
    if want_lines && metadata.is_file() {
        match count_lines(path) {
            Some(lines) => measure.lines = lines,
            None => measure.unreadable += 1,
        }
    }
    measure
}

fn count_lines(path: &Path) -> Option<u64> {
    let mut file = fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; 256 * 1024];
    let mut lines = 0u64;
    let mut first = true;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            return Some(lines);
        }
        // A NUL early in the file marks it binary; newlines in binary data
        // are noise, not lines.
        if first && buffer[..read].contains(&0) {
            return Some(0);
        }
        first = false;
        lines += buffer[..read].iter().filter(|byte| **byte == b'\n').count() as u64;
    }
}

/// Depth-first aggregate of everything under `directory`, hidden included,
/// with every directory's entries processed in parallel. Rows are only
/// recorded down to the display depth and, unless `-a`, only for visible
/// entries — an invisible directory hides its whole subtree from the listing
/// while still counting toward every total.
fn walk_directory(
    options: &Options,
    directory: &Path,
    relative: &Path,
    depth: usize,
) -> (Measure, Vec<Row>) {
    let unreadable = || {
        (
            Measure {
                unreadable: 1,
                ..Measure::default()
            },
            Vec::new(),
        )
    };
    let entries: Vec<fs::DirEntry> = match fs::read_dir(directory) {
        Ok(entries) => entries.flatten().collect(),
        Err(_) => return unreadable(),
    };
    entries
        .into_par_iter()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_relative = relative.join(&name);
            let Ok(metadata) = entry.metadata() else {
                return unreadable();
            };
            let visible = depth < options.display_depth && (options.all || !hidden(&name));
            let (measure, mut rows) = if metadata.is_dir() {
                // A hidden directory's children stay out of the listing even
                // with room left in the depth budget, so cap their display
                // depth.
                let child_depth = if visible {
                    depth + 1
                } else {
                    options.display_depth
                };
                walk_directory(options, &entry.path(), &child_relative, child_depth)
            } else {
                (
                    measure_file(&entry.path(), &metadata, options.lines),
                    Vec::new(),
                )
            };
            if visible {
                rows.push(Row {
                    name: child_relative.to_string_lossy().to_string(),
                    kind: kind_of(&metadata),
                    executable: is_executable(&metadata),
                    measure,
                });
            }
            (measure, rows)
        })
        .reduce(
            || (Measure::default(), Vec::new()),
            |mut left, right| {
                left.0.add(right.0);
                left.1.extend(right.1);
                (left.0, left.1)
            },
        )
}

/// Files and links first, directories last — each group largest first.
fn sort_rows(rows: &mut [Row], lines: bool) {
    rows.sort_by(|a, b| {
        let group = |row: &Row| usize::from(row.kind == "directory");
        let metric = |row: &Row| {
            if lines {
                row.measure.lines
            } else {
                row.measure.bytes
            }
        };
        group(a)
            .cmp(&group(b))
            .then(metric(b).cmp(&metric(a)))
            .then(a.name.cmp(&b.name))
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

/// The colors ls would use: LS_COLORS entries when set, GNU defaults when not.
fn ls_color(row: &Row) -> Option<String> {
    let table = std::env::var("LS_COLORS").unwrap_or_default();
    let lookup = |key: &str| {
        table.split(':').find_map(|entry| {
            entry
                .strip_prefix(key)?
                .strip_prefix('=')
                .map(str::to_string)
        })
    };
    match row.kind {
        "directory" => Some(lookup("di").unwrap_or_else(|| "01;34".to_string())),
        "link" => Some(lookup("ln").unwrap_or_else(|| "01;36".to_string())),
        _ if row.executable => Some(lookup("ex").unwrap_or_else(|| "01;32".to_string())),
        _ => lookup("fi"),
    }
}

/// The Nerd Font glyph eza would show for this entry.
fn icon_for(row: &Row) -> char {
    if row.kind == "directory" {
        return '\u{f115}';
    }
    let base = row
        .name
        .rsplit('/')
        .next()
        .unwrap_or(&row.name)
        .to_lowercase();
    match base.as_str() {
        "dockerfile" => return '\u{e650}',
        "makefile" | "justfile" => return '\u{e673}',
        "license" | "license.md" | "license.txt" => return '\u{f02d}',
        "readme" | "readme.md" => return '\u{f00ba}',
        _ => {}
    }
    if base.starts_with(".git") {
        return '\u{f02a2}';
    }
    let extension = match base.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    };
    match extension {
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
    let styled = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
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
    let value_width = rows
        .iter()
        .map(|row| plain_value(row.measure, lines).chars().count())
        .chain([value_header.len(), total_text.chars().count()])
        .max()
        .unwrap_or(4);

    for row in rows {
        let padding = " ".repeat(name_width - row.name.chars().count() - prefix);
        let name = if styled {
            let icon = icon_for(row);
            match ls_color(row) {
                Some(color) => format!("\x1b[{color}m{icon} {}{reset}", row.name),
                None => format!("{icon} {}", row.name),
            }
        } else {
            row.name.clone()
        };
        println!(
            "{name}{padding}  {:>value_width$}",
            plain_value(row.measure, lines)
        );
    }
    let width = name_width + value_width + 2;
    println!("{dim}{}{reset}", "─".repeat(width));
    println!(
        "{bold}{:<name_width$}  {:>value_width$}{reset}",
        "Total", total_text
    );
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

    fn walk_all(root: &Path, lines: bool, all: bool, depth: usize) -> (Vec<Row>, Measure) {
        let options = Options {
            lines,
            all,
            display_depth: depth,
        };
        let (total, mut rows) = walk_directory(&options, root, Path::new(""), 0);
        sort_rows(&mut rows, lines);
        (rows, total)
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
        assert_eq!(rows[0].name, "tiny.txt");
    }

    #[test]
    fn files_sort_before_directories_largest_first() {
        let root = tree();
        fs::write(root.path().join("big.txt"), vec![b'x'; 9000]).unwrap();
        let (rows, _) = walk_all(root.path(), false, false, 1);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["big.txt", "notes.txt", "assets"]);
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
    fn icons_follow_the_eza_table() {
        assert_eq!(icon_for(&row("src", "directory")), '\u{f115}');
        assert_eq!(icon_for(&row("main.rs", "file")), '\u{e68b}');
        assert_eq!(icon_for(&row("deep/path/notes.md", "file")), '\u{f48a}');
        assert_eq!(icon_for(&row("README.md", "file")), '\u{f00ba}');
        assert_eq!(icon_for(&row(".gitignore", "file")), '\u{f02a2}');
        assert_eq!(icon_for(&row("mystery", "file")), '\u{f086f}');
    }
}
