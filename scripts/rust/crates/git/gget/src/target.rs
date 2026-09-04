const HOST: &str = "github.com";

#[derive(Debug)]
pub struct Target {
    pub owner: String,
    pub repo: String,
    pub reference: Option<String>,
    pub path: String,
}

impl Target {
    pub fn url(&self) -> String {
        format!("https://{HOST}/{}/{}", self.owner, self.repo)
    }

    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn name(&self) -> &str {
        match self.path.rsplit('/').next() {
            Some(name) if !name.is_empty() => name,
            _ => &self.repo,
        }
    }
}

pub fn parse(input: &str, owner: Option<&str>) -> Result<Target, String> {
    // A browser hands over the lines it is scrolled to and the file view adds
    // `?plain=1`; neither says anything about which file it is.
    let text = input.split(['#', '?']).next().unwrap_or(input).trim();
    let own = owner.is_some();
    let rest = match owner {
        Some(_) if addressed(text) => {
            return Err(format!("{text}: --fredrir takes a repository, not a URL"));
        }
        Some(_) => text,
        None => match locate(text)? {
            Some(path) => path,
            None => text,
        },
    };

    let mut parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    if parts.iter().any(|part| *part == "." || *part == "..") {
        return Err(format!("{text}: . and .. are not places in a repository"));
    }
    let owner = match owner {
        Some(fixed) => fixed.to_string(),
        None if parts.is_empty() => return Err(expected(text, own)),
        None => parts.remove(0).to_string(),
    };
    if parts.is_empty() {
        return Err(expected(text, own));
    }
    // `git@github.com:owner/repo.git` is the same repository as the one the
    // browser shows without the suffix.
    let repo = parts.remove(0);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if repo.is_empty() || owner.is_empty() {
        return Err(expected(text, own));
    }
    // `tree` and `blob` are GitHub's own markers for "a commit comes next",
    // so the segment behind one is a branch, a tag or a commit rather than a
    // folder. One segment, since a branch name may hold slashes and only the
    // repository could say where such a name ends: `-b` is for those.
    let mut reference = None;
    if parts.len() > 1 && matches!(parts[0], "tree" | "blob") {
        parts.remove(0);
        reference = Some(parts.remove(0).to_string());
    }
    Ok(Target {
        owner,
        repo: repo.to_string(),
        reference,
        path: parts.join("/"),
    })
}

fn addressed(text: &str) -> bool {
    text.contains("://")
        || text.starts_with("git@")
        || text.to_ascii_lowercase().starts_with(HOST)
        || text.to_ascii_lowercase().starts_with("www.")
}

fn locate(text: &str) -> Result<Option<&str>, String> {
    // The scp-like form a clone box offers for ssh, with a colon where the
    // path would otherwise start.
    if let Some(rest) = text.strip_prefix("git@") {
        let (authority, path) = rest.split_once(':').ok_or_else(|| elsewhere(text))?;
        return github(authority, path, text).map(Some);
    }
    let rest = ["https://", "http://", "ssh://", "git://"]
        .into_iter()
        .find_map(|scheme| text.strip_prefix(scheme))
        .unwrap_or(text);
    let (authority, path) = match rest.split_once('/') {
        Some(split) => split,
        // A scheme and nothing else is still an address, and a bare word with
        // no scheme is still shorthand.
        None if rest == text => return Ok(None),
        None => (rest, ""),
    };
    // A GitHub account name cannot hold a dot and a host cannot do without
    // one, which is the whole difference between `github.com/user/repo` and
    // `user/repo`.
    if rest == text && !authority.contains('.') {
        return Ok(None);
    }
    github(authority, path, text).map(Some)
}

fn github<'a>(authority: &str, path: &'a str, text: &str) -> Result<&'a str, String> {
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    if !host.eq_ignore_ascii_case(HOST) {
        return Err(elsewhere(text));
    }
    Ok(path)
}

fn elsewhere(text: &str) -> String {
    format!("{text}: not a {HOST} address")
}

fn expected(text: &str, own: bool) -> String {
    if own {
        format!("{text}: expected a repository, as in nsql/README.md")
    } else {
        format!("{text}: expected owner/repo, as in user/repo/tree/main/src")
    }
}

#[cfg(test)]
#[path = "../tests/unit/target_tests.rs"]
mod tests;
