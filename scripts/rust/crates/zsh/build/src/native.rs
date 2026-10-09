//! Native zsh for system commands whose output depends on the runtime
//! environment: their fixed inputs are read now, the rest is left to run.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::quote;

/// The command each emulation replaces, by file name and arguments.
pub fn emulates(argv: &[String]) -> bool {
    program(argv) == Some("path_helper") && argv[1..] == ["-s"]
}

fn program(argv: &[String]) -> Option<&str> {
    argv.first()
        .and_then(|program| Path::new(program).file_name())
        .and_then(|name| name.to_str())
}

/// A command substitution whose output native zsh makes without a process.
pub struct Substitute {
    /// The command, by file name and arguments.
    pub argv: &'static [&'static str],
    /// Defines `function`, a math function that sets `value` or empties it.
    pub helper: &'static str,
    pub function: &'static str,
    pub value: &'static str,
    /// Whether the command's output has the form `helper` makes.
    pub matches: fn(&str) -> bool,
}

const SUBSTITUTES: &[Substitute] = &[Substitute {
    argv: &["atuin", "uuid"],
    helper: include_str!("../assets/uuid7.zsh"),
    function: "_zsh_build_uuid7",
    value: "_zsh_build_uuid",
    matches: |output| is_uuid7(output, SystemTime::now()),
}];

pub fn substitute(argv: &[String]) -> Option<&'static Substitute> {
    let name = program(argv)?;
    SUBSTITUTES
        .iter()
        .find(|substitute| substitute.argv[0] == name && substitute.argv[1..] == argv[1..])
}

impl Substitute {
    /// The helper's value, else the output of `original`, the substitution it replaces.
    pub fn expansion(&self, original: &str) -> String {
        // No spaces in the math call: `sh_word_split` splits it inside `${:-...}`.
        format!(
            "${{${{:-$(({function}()))}}:+${{{value}:-{original}}}}}",
            function = self.function,
            value = self.value,
        )
    }
}

/// `atuin uuid` output: a UUIDv7 made now, in simple form, with the `uuid`
/// crate's 42-bit counter in a new process, seeded below 2^41.
pub fn is_uuid7(output: &str, now: SystemTime) -> bool {
    let Some(id) = output.strip_suffix('\n') else {
        return false;
    };
    let bytes = id.as_bytes();
    if bytes.len() != 32
        || !bytes
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return false;
    }
    let Ok(millis) = u64::from_str_radix(&id[..12], 16) else {
        return false;
    };
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    });
    bytes[12] == b'7'
        && bytes[13] <= b'7'
        && matches!(bytes[16], b'8' | b'9' | b'a' | b'b')
        && millis.abs_diff(now) < 60_000
}

/// `path_helper -s`: `/etc/paths` entries, then those of `/etc/paths.d` in
/// its sort order, then the current entries not yet listed; the same for
/// `MANPATH` when it is exported.
#[derive(Debug, PartialEq, Eq)]
pub struct PathHelper {
    pub path: Vec<String>,
    pub manpath: Vec<String>,
    /// Files and directories whose change makes this stale.
    pub sources: Vec<PathBuf>,
}

impl PathHelper {
    pub fn load(root: &str) -> Result<Self, String> {
        let mut sources = vec![PathBuf::from(format!("{root}/etc"))];
        let path = entries(root, "/etc/paths", "/etc/paths.d", &mut sources)?;
        let manpath = entries(root, "/etc/manpaths", "/etc/manpaths.d", &mut sources)?;
        Ok(Self {
            path,
            manpath,
            sources,
        })
    }

    /// Replaces `original`, which still runs when the environment holds
    /// what `eval` would reinterpret or `path_helper` reads differently.
    pub fn code(&self, original: &str) -> String {
        let original = original.trim_end_matches('\n');
        format!(
            r#"if [[ -n ${{PATH_HELPER_ROOT+1}} || $IFS != $' \t\n\0' || ${{(t)PATH}} != *export* || $PATH$MANPATH == *[\"\$\\\`$'\t\n']* || $PATH$MANPATH == *'  '* ]]; then
{original}
else
() {{
local -a entries=({path})
local entry
for entry in "${{(@s.:.)PATH}}"; do
[[ -n $entry && ${{entries[(Ie)$entry]}} == 0 ]] && entries+=("$entry")
done
PATH=${{(j.:.)entries}}
export PATH
[[ ${{(t)MANPATH}} == *export* ]] || return 0
entries=({manpath})
for entry in "${{(@s.:.)MANPATH}}"; do
[[ -n $entry && ${{entries[(Ie)$entry]}} == 0 ]] && entries+=("$entry")
done
MANPATH=${{(j.:.)entries}}:
export MANPATH
}}
fi
"#,
            path = words(&self.path),
            manpath = words(&self.manpath),
        )
    }
}

fn words(values: &[String]) -> String {
    values
        .iter()
        .map(|value| quote::word(value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn entries(
    root: &str,
    defaults: &str,
    directory: &str,
    sources: &mut Vec<PathBuf>,
) -> Result<Vec<String>, String> {
    let defaults = PathBuf::from(format!("{root}{defaults}"));
    let directory = PathBuf::from(format!("{root}{directory}"));
    sources.push(defaults.clone());
    sources.push(directory.clone());
    let mut files = Vec::new();
    if is_regular(&defaults) {
        files.push(defaults.clone());
    } else if defaults.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
        return Err(format!("{}: a directory", defaults.display()));
    }
    if directory.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| format!("{}: unsupported name {name:?}", directory.display()))?;
            names.push(name);
        }
        sort_names(&mut names)
            .map_err(|name| format!("{}/{name}: unsupported name", directory.display()))?;
        for name in names {
            let file = directory.join(name);
            if is_regular(&file) {
                sources.push(file.clone());
                files.push(file);
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    for file in files {
        let bytes = std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let entry = std::str::from_utf8(line)
                .ok()
                .filter(|entry| entry.bytes().all(quote::is_plain) && !entry.contains(':'))
                .ok_or_else(|| format!("{}: unsupported entry", file.display()))?;
            if !out.iter().any(|known| known == entry) {
                out.push(entry.to_string());
            }
        }
    }
    Ok(out)
}

fn is_regular(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|meta| meta.is_file())
}

/// `path_helper`'s order: by numeric prefix when both names have one, else
/// bytewise. Names that order inconsistently under it are refused.
pub fn sort_names(names: &mut [String]) -> Result<(), String> {
    if let Some(name) = names.iter().find(|name| {
        name.starts_with(|c: char| c.is_ascii_whitespace() || c == '+' || c == '-')
            || name.bytes().take_while(u8::is_ascii_digit).count() > 18
    }) {
        return Err(name.clone());
    }
    names.sort_by(|left, right| compare(left, right));
    match names
        .windows(2)
        .find(|pair| compare(&pair[0], &pair[1]) == Ordering::Equal)
    {
        Some(pair) => Err(pair[1].clone()),
        None => Ok(()),
    }
}

fn compare(left: &str, right: &str) -> Ordering {
    match (numeric(left), numeric(right)) {
        (Some((left, left_rest)), Some((right, right_rest))) => left
            .cmp(&right)
            .then_with(|| left_rest.as_bytes().cmp(right_rest.as_bytes())),
        _ => left.as_bytes().cmp(right.as_bytes()),
    }
}

fn numeric(name: &str) -> Option<(u64, &str)> {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    let value = name[..digits].parse().ok()?;
    Some((value, &name[digits..]))
}

#[cfg(test)]
#[path = "../tests/unit/native_tests.rs"]
mod tests;
