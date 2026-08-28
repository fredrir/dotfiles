use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The program that answers, which is also the one that does the work.
pub const PROGRAM: &str = "dotfmt";

/// What dotfmt had to say about the candidates.
pub enum Owned {
    /// The subset it claims, in the order the candidates were offered.
    Claimed(Vec<PathBuf>),
    /// dotfmt is not on `PATH`. A fact about this machine, never a failure —
    /// the same answer every other provider gets.
    Missing,
    /// dotfmt is installed and could not answer. This one is a failure: a
    /// dotfmt too old to know `--owns` would otherwise silently own nothing,
    /// and a run that formats none of the `.conf` files while reporting
    /// success is exactly the shape of bug this call exists to close.
    Failed(String),
}

pub fn ask(root: &Path, files: &[PathBuf]) -> Owned {
    if files.is_empty() {
        return Owned::Claimed(Vec::new());
    }
    let child = Command::new(PROGRAM)
        .arg("--owns")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Owned::Missing,
        Err(error) => return Owned::Failed(format!("{PROGRAM}: {error}\n")),
    };

    let Some(mut sink) = child.stdin.take() else {
        return Owned::Failed(format!("{PROGRAM}: stdin is not a pipe\n"));
    };
    let payload = joined(files);
    let feeding = std::thread::spawn(move || sink.write_all(&payload));

    let mut answer = Vec::new();
    if let Some(mut source) = child.stdout.take() {
        source.read_to_end(&mut answer).ok();
    }
    let fed = feeding.join();
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => return Owned::Failed(format!("{PROGRAM}: {error}\n")),
    };
    // A short write means dotfmt stopped reading, so the answer is about some
    // prefix of the question rather than about the question.
    if !matches!(fed, Ok(Ok(()))) {
        return Owned::Failed(format!("{PROGRAM} --owns: the file list was not read\n"));
    }
    if !output.status.success() {
        let mut said = String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_string();
        if said.is_empty() {
            said = format!("{PROGRAM} --owns: exited {}", show(&output.status));
        }
        said.push('\n');
        return Owned::Failed(said);
    }
    Owned::Claimed(split(&answer, files))
}

fn joined(files: &[PathBuf]) -> Vec<u8> {
    files
        .iter()
        .flat_map(|path| {
            let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
            bytes.push(0);
            bytes
        })
        .collect()
}

fn split(answer: &[u8], asked: &[PathBuf]) -> Vec<PathBuf> {
    let offered: HashSet<&[u8]> = asked
        .iter()
        .map(|path| path.as_os_str().as_encoded_bytes())
        .collect();
    let mut claimed: HashSet<Vec<u8>> = answer
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty() && offered.contains(path))
        .map(<[u8]>::to_vec)
        .collect();
    // Back into the order the candidates were offered in, so two runs over one
    // tree hand dotfmt the same command line.
    asked
        .iter()
        .filter(|path| claimed.remove(path.as_os_str().as_encoded_bytes()))
        .cloned()
        .collect()
}

fn show(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "on a signal".to_string(),
    }
}
