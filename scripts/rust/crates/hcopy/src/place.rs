use std::path::{Component, Path, PathBuf};

// The local side of a transfer, which is always somewhere under this home.
#[derive(Debug)]
pub struct Local {
    pub absolute: PathBuf,
    pub relative: String,
    pub name: String,
}

impl Local {
    pub fn display(&self) -> String {
        format!("~/{}", self.relative)
    }

    pub fn parent(&self) -> String {
        match self.relative.rsplit_once('/') {
            Some((head, _)) => head.to_string(),
            None => String::new(),
        }
    }
}

pub fn home() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let home = PathBuf::from(home);
    Ok(std::fs::canonicalize(&home).unwrap_or(home))
}

pub fn absolute(input: &str, home: &Path) -> Result<PathBuf, String> {
    let expanded = expand(input, home);
    let rooted = match expanded.is_absolute() {
        true => expanded,
        false => {
            let here = std::env::current_dir()
                .map_err(|error| format!("this directory is gone: {error}"))?;
            here.join(expanded)
        }
    };
    // A path being pulled does not exist yet, so the symlinks that do resolve
    // are followed and the rest of the path is normalised by hand.
    Ok(std::fs::canonicalize(&rooted).unwrap_or_else(|_| tidy(&rooted)))
}

pub fn resolve(input: &str, home: &Path) -> Result<Local, String> {
    let absolute = absolute(input, home)?;
    if absolute == home {
        return Err("that is your whole home directory; name a path inside it".to_string());
    }
    let relative = absolute
        .strip_prefix(home)
        .ok()
        .and_then(|rest| rest.to_str())
        .filter(|rest| !rest.is_empty())
        .ok_or_else(|| {
            format!(
                "path must be inside your home directory: {}",
                absolute.display()
            )
        })?
        .to_string();

    let name = relative
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("path has no name: {}", absolute.display()))?
        .to_string();

    Ok(Local {
        absolute,
        relative,
        name,
    })
}

fn expand(input: &str, home: &Path) -> PathBuf {
    match input {
        "~" => home.to_path_buf(),
        _ => match input.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(input),
        },
    }
}

fn tidy(path: &Path) -> PathBuf {
    let mut kept = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                kept.pop();
            }
            other => kept.push(other),
        }
    }
    kept
}

// A remote path is only ever handed to a shell on the other machine, so it is
// text here rather than a PathBuf: this machine's rules do not apply to it.
pub fn join(directory: &str, name: &str) -> String {
    match directory.ends_with('/') {
        true => format!("{directory}{name}"),
        false => format!("{directory}/{name}"),
    }
}

pub fn parent_of(path: &str) -> &str {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/",
        Some((head, _)) => head,
        None => "/",
    }
}

pub fn name_of(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

pub fn expand_remote(input: &str, home: &str) -> String {
    let expanded = match input {
        "~" => home.to_string(),
        _ => match input.strip_prefix("~/") {
            Some(rest) => join(home, rest),
            None => input.to_string(),
        },
    };
    match expanded.starts_with('/') {
        true => expanded,
        false => join(home, &expanded),
    }
}

// Where a pulled path lands: the same place under this home when it came
// from under that one, and otherwise here, where it was asked for.
pub fn landing(
    remote: &str,
    remote_home: &str,
    home: &Path,
    here: &Path,
) -> Result<(PathBuf, String), String> {
    if let Some(rest) = remote.strip_prefix(&format!("{remote_home}/")) {
        return Ok((home.join(rest), format!("~/{rest}")));
    }
    let landed = here.join(name_of(remote));
    let shown = landed
        .strip_prefix(home)
        .map(|rest| format!("~/{}", rest.display()))
        .map_err(|_| {
            format!(
                "a path from outside that home lands here, which is outside this one: {}",
                landed.display()
            )
        })?;
    Ok((landed, shown))
}

#[cfg(test)]
#[path = "../tests/unit/place_tests.rs"]
mod tests;
