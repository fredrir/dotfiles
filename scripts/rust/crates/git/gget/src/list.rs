use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use crate::fetch::{self, Outcome};
use crate::target::Target;

enum Kind {
    Directory,
    File { executable: bool },
    Link,
}

struct Entry {
    name: String,
    kind: Kind,
}

struct Lister {
    program: &'static str,
    icons: Option<&'static str>,
    hidden: &'static str,
}

pub fn list(target: &Target, all: bool) -> Result<ExitCode, String> {
    let temp = tempfile::Builder::new()
        .prefix(".gget-")
        .tempdir()
        .map_err(|error| format!("a temporary directory: {error}"))?;
    let clone = temp.path().join("clone");
    let at = fetch::text(&clone)?;
    let url = target.url();

    let mut arguments = vec![
        "clone",
        "--quiet",
        "--depth",
        "1",
        "--filter=tree:0",
        "--no-checkout",
    ];
    if let Some(reference) = &target.reference {
        arguments.extend(["--branch", reference]);
    }
    arguments.extend([url.as_str(), at]);
    if let Some(Outcome::Refused(code)) = fetch::run(&arguments)? {
        return Ok(ExitCode::from(crate::byte(code)));
    }

    let branch = fetch::branch(&clone);
    let source = fetch::reported(target, branch.as_deref());
    let revision = format!("HEAD:{}", target.path);
    let kind = capture(&["-C", at, "cat-file", "-t", &revision])
        .ok_or_else(|| format!("no {} in {source}", target.path))?;

    let entries = if kind.trim() == "tree" {
        read(at, &revision).ok_or_else(|| format!("{source}: the listing could not be read"))?
    } else {
        vec![Entry {
            name: target.name().to_string(),
            kind: Kind::File { executable: false },
        }]
    };

    let shown = temp.path().join("listing");
    fs::create_dir(&shown).map_err(|error| format!("{}: {error}", shown.display()))?;
    for entry in &entries {
        place(&shown, entry, at, &revision)?;
    }
    show(&shown, all)
}

fn read(at: &str, revision: &str) -> Option<Vec<Entry>> {
    let listed = capture(&["-C", at, "ls-tree", "-z", revision])?;
    Some(listed.split('\0').filter_map(entry).collect())
}

fn entry(record: &str) -> Option<Entry> {
    let (about, name) = record.split_once('\t')?;
    let mode = about.split(' ').next()?;
    let kind = match mode {
        "040000" | "160000" => Kind::Directory,
        "120000" => Kind::Link,
        _ => Kind::File {
            executable: mode.ends_with("755"),
        },
    };
    Some(Entry {
        name: name.to_string(),
        kind,
    })
}

fn place(root: &Path, entry: &Entry, at: &str, revision: &str) -> Result<(), String> {
    let path = root.join(&entry.name);
    let made = match entry.kind {
        Kind::Directory => fs::create_dir(&path),
        Kind::File { executable } => file(&path, executable),
        Kind::Link => link(&path, &inside(revision, &entry.name), at),
    };
    made.map_err(|error| format!("{}: {error}", path.display()))
}

fn inside(revision: &str, name: &str) -> String {
    let separator = if revision.ends_with(':') { "" } else { "/" };
    format!("{revision}{separator}{name}")
}

fn file(path: &Path, executable: bool) -> io::Result<()> {
    fs::File::create(path)?;
    if executable {
        mark(path)?;
    }
    Ok(())
}

fn link(path: &Path, revision: &str, at: &str) -> io::Result<()> {
    let target = capture(&["-C", at, "cat-file", "blob", revision]).unwrap_or_default();
    match target.trim_end_matches('\n') {
        "" => file(path, false),
        target => symlink(target, path),
    }
}

#[cfg(unix)]
fn symlink(target: &str, path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(not(unix))]
fn symlink(_target: &str, path: &Path) -> io::Result<()> {
    file(path, false)
}

#[cfg(unix)]
fn mark(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn mark(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn show(root: &Path, all: bool) -> Result<ExitCode, String> {
    let lister = lister();
    let mut arguments: Vec<&str> = Vec::new();
    arguments.extend(lister.icons);
    if all {
        arguments.push(lister.hidden);
    }
    let status = Command::new(lister.program)
        .args(arguments)
        .arg(root)
        .status()
        .map_err(|error| format!("{}: {error}", lister.program))?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        code => ExitCode::from(crate::byte(code.unwrap_or(1))),
    })
}

fn lister() -> Lister {
    if available("eza") {
        return Lister {
            program: "eza",
            icons: Some("--icons=auto"),
            hidden: "-a",
        };
    }
    Lister {
        program: "ls",
        icons: None,
        hidden: "-A",
    }
}

fn available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn capture(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_read_as_its_mode_says() {
        let kinds: Vec<Kind> = [
            "040000 tree abc\tfolder_8",
            "100644 blob abc\tREADME.md",
            "100755 blob abc\tsetup.sh",
            "120000 blob abc\tlink",
            "160000 commit abc\tvendor",
        ]
        .into_iter()
        .filter_map(entry)
        .map(|entry| entry.kind)
        .collect();
        assert!(matches!(
            kinds.as_slice(),
            [
                Kind::Directory,
                Kind::File { executable: false },
                Kind::File { executable: true },
                Kind::Link,
                Kind::Directory,
            ]
        ));
    }

    #[test]
    fn a_name_is_whatever_follows_the_tab() {
        let named = entry("100644 blob abc\ta name with spaces.md").expect("the record is read");
        assert_eq!(named.name, "a name with spaces.md");
        assert!(entry("").is_none());
    }

    #[test]
    fn an_entry_of_the_root_needs_no_separator() {
        assert_eq!(inside("HEAD:", "link"), "HEAD:link");
        assert_eq!(inside("HEAD:config", "link"), "HEAD:config/link");
    }
}
