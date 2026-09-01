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

// `~/projects` reads better than `/home/fredrir/projects` and is what every
// other message about these two machines already says.
pub fn shorten(path: &str, home: &str) -> String {
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => path.to_string(),
    }
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

pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Nowhere real, so that resolving never reaches this machine's own
    // filesystem and the lexical rules are what is under test.
    fn home_path() -> PathBuf {
        PathBuf::from("/nowhere/fredrir")
    }

    #[test]
    fn a_tilde_is_this_home() {
        assert_eq!(expand("~", &home_path()), home_path());
        assert_eq!(
            expand("~/projects/go", &home_path()),
            PathBuf::from("/nowhere/fredrir/projects/go")
        );
    }

    #[test]
    fn a_tilde_only_leads_a_path() {
        assert_eq!(expand("a~b", &home_path()), PathBuf::from("a~b"));
        assert_eq!(expand("./~", &home_path()), PathBuf::from("./~"));
    }

    #[test]
    fn tidy_removes_the_steps_that_go_nowhere() {
        assert_eq!(
            tidy(Path::new("/Users/fredrir/./projects/../go")),
            PathBuf::from("/Users/fredrir/go")
        );
    }

    #[test]
    fn home_itself_is_not_a_path_to_copy() {
        let error = resolve("~", &home_path()).unwrap_err();
        assert!(error.contains("whole home directory"), "{error}");
    }

    #[test]
    fn a_path_outside_home_is_refused() {
        let error = resolve("/etc/hosts", &home_path()).unwrap_err();
        assert!(error.contains("inside your home directory"));
    }

    #[test]
    fn a_symlink_is_followed_to_what_it_points_at() {
        let root = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(root.path()).unwrap();
        std::fs::create_dir_all(home.join("dotfiles/tmux")).unwrap();
        std::fs::write(home.join("dotfiles/tmux/tmux.conf"), "").unwrap();
        std::os::unix::fs::symlink(
            home.join("dotfiles/tmux/tmux.conf"),
            home.join(".tmux.conf"),
        )
        .unwrap();

        let local = resolve(&home.join(".tmux.conf").to_string_lossy(), &home).unwrap();
        assert_eq!(local.relative, "dotfiles/tmux/tmux.conf");
        assert_eq!(local.name, "tmux.conf");
    }

    #[test]
    fn a_path_that_is_not_there_yet_is_still_resolved() {
        let root = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(root.path()).unwrap();
        let local = resolve(&home.join("not/here/yet").to_string_lossy(), &home).unwrap();
        assert_eq!(local.relative, "not/here/yet");
        assert_eq!(local.name, "yet");
    }

    #[test]
    fn a_resolved_path_knows_its_own_shape() {
        let local = resolve("~/projects/my-app", &home_path()).unwrap();
        assert_eq!(local.relative, "projects/my-app");
        assert_eq!(local.name, "my-app");
        assert_eq!(local.parent(), "projects");
        assert_eq!(local.display(), "~/projects/my-app");
    }

    #[test]
    fn a_path_directly_in_home_has_no_parent_below_it() {
        let local = resolve("~/.tmux.conf", &home_path()).unwrap();
        assert_eq!(local.relative, ".tmux.conf");
        assert_eq!(local.parent(), "");
    }

    #[test]
    fn remote_paths_are_joined_without_doubling_the_separator() {
        assert_eq!(join("/home/f", "go"), "/home/f/go");
        assert_eq!(join("/home/f/", "go"), "/home/f/go");
        assert_eq!(join("/", "go"), "/go");
    }

    #[test]
    fn a_remote_path_can_be_taken_apart() {
        assert_eq!(parent_of("/home/f/projects/go"), "/home/f/projects");
        assert_eq!(name_of("/home/f/projects/go"), "go");
        assert_eq!(parent_of("/home"), "/");
        assert_eq!(parent_of("/"), "/");
        assert_eq!(name_of("/home/f/go/"), "go");
    }

    #[test]
    fn a_remote_home_is_shown_as_a_tilde() {
        assert_eq!(shorten("/home/f/go", "/home/f"), "~/go");
        assert_eq!(shorten("/home/f", "/home/f"), "~");
        assert_eq!(shorten("/etc", "/home/f"), "/etc");
        assert_eq!(shorten("/home/fredrir2/go", "/home/f"), "/home/fredrir2/go");
    }

    #[test]
    fn a_typed_remote_path_is_resolved_against_the_remote_home() {
        assert_eq!(expand_remote("~/go", "/home/f"), "/home/f/go");
        assert_eq!(expand_remote("~", "/home/f"), "/home/f");
        assert_eq!(expand_remote("/etc", "/home/f"), "/etc");
        assert_eq!(expand_remote("go", "/home/f"), "/home/f/go");
    }

    #[test]
    fn a_pulled_path_from_that_home_lands_in_the_same_place_under_this_one() {
        let (landed, shown) = landing(
            "/home/fredrir/projects/my-app",
            "/home/fredrir",
            Path::new("/Users/fredrir"),
            Path::new("/Users/fredrir/somewhere"),
        )
        .unwrap();
        assert_eq!(landed, PathBuf::from("/Users/fredrir/projects/my-app"));
        assert_eq!(shown, "~/projects/my-app");
    }

    #[test]
    fn a_pulled_path_from_outside_that_home_lands_where_it_was_asked_for() {
        let (landed, shown) = landing(
            "/etc/ssh/sshd_config",
            "/home/fredrir",
            Path::new("/Users/fredrir"),
            Path::new("/Users/fredrir/notes"),
        )
        .unwrap();
        assert_eq!(landed, PathBuf::from("/Users/fredrir/notes/sshd_config"));
        assert_eq!(shown, "~/notes/sshd_config");
    }

    #[test]
    fn a_pull_into_somewhere_outside_this_home_is_refused() {
        let error = landing(
            "/etc/ssh/sshd_config",
            "/home/fredrir",
            Path::new("/Users/fredrir"),
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(error.contains("outside this one"));
    }

    #[test]
    fn a_home_that_only_shares_a_prefix_is_not_that_home() {
        let (landed, _) = landing(
            "/home/fredrir2/notes",
            "/home/fredrir",
            Path::new("/Users/fredrir"),
            Path::new("/Users/fredrir"),
        )
        .unwrap();
        assert_eq!(landed, PathBuf::from("/Users/fredrir/notes"));
    }

    #[test]
    fn quoting_survives_a_name_with_a_quote_in_it() {
        assert_eq!(quote("/home/f/go"), "'/home/f/go'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("a b"), "'a b'");
    }
}
