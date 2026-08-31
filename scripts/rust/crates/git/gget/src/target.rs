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
mod tests {
    use super::*;

    fn read(input: &str) -> Target {
        parse(input, None).expect("the target is read")
    }

    fn mine(input: &str) -> Target {
        parse(input, Some("fredrir")).expect("the target is read")
    }

    fn shape(target: &Target) -> (String, Option<&str>, &str, &str) {
        (
            target.slug(),
            target.reference.as_deref(),
            target.path.as_str(),
            target.name(),
        )
    }

    #[test]
    fn a_url_without_a_marker_is_the_default_branch() {
        let target = read("https://github.com/user/repo/folder_8/folder_9");
        assert_eq!(
            shape(&target),
            ("user/repo".into(), None, "folder_8/folder_9", "folder_9")
        );
    }

    #[test]
    fn a_tree_marker_names_the_branch() {
        let target = read("https://github.com/user/repo/tree/dev/folder_8/folder_10");
        assert_eq!(
            shape(&target),
            (
                "user/repo".into(),
                Some("dev"),
                "folder_8/folder_10",
                "folder_10"
            )
        );
    }

    #[test]
    fn a_blob_marker_names_it_the_same_way() {
        let target = read("https://github.com/user/repo/blob/dev/README.md");
        assert_eq!(
            shape(&target),
            ("user/repo".into(), Some("dev"), "README.md", "README.md")
        );
    }

    #[test]
    fn the_repository_itself_is_named_after_the_repository() {
        let target = read("https://github.com/user/repo");
        assert_eq!(shape(&target), ("user/repo".into(), None, "", "repo"));
    }

    #[test]
    fn the_owner_flag_stands_in_for_the_first_segment() {
        let target = mine("nsql/README.md");
        assert_eq!(
            shape(&target),
            ("fredrir/nsql".into(), None, "README.md", "README.md")
        );
    }

    #[test]
    fn a_middle_segment_is_part_of_the_path() {
        let target = mine("nsql/dev/README.md");
        assert_eq!(
            shape(&target),
            ("fredrir/nsql".into(), None, "dev/README.md", "README.md")
        );
    }

    #[test]
    fn every_address_form_names_the_same_place() {
        let same = [
            "https://github.com/user/repo/tree/main/src",
            "http://www.github.com/user/repo/tree/main/src",
            "github.com/user/repo/tree/main/src",
            "git@github.com:user/repo.git/tree/main/src",
            "ssh://git@github.com/user/repo/tree/main/src",
            "https://github.com/user/repo/tree/main/src?plain=1#L4",
            "user/repo/tree/main/src",
        ];
        for input in same {
            let target = read(input);
            assert_eq!(
                (target.url(), target.reference.as_deref(), target.path),
                (
                    "https://github.com/user/repo".to_string(),
                    Some("main"),
                    "src".to_string()
                ),
                "{input}"
            );
        }
    }

    #[test]
    fn a_lone_tree_is_a_folder_called_tree() {
        let target = read("user/repo/tree");
        assert_eq!(shape(&target), ("user/repo".into(), None, "tree", "tree"));
    }

    #[test]
    fn another_host_is_refused() {
        for input in [
            "https://gitlab.com/user/repo",
            "git@gitlab.com:user/repo.git",
            "https://raw.githubusercontent.com/user/repo/main/README.md",
        ] {
            let error = parse(input, None).expect_err("the host is refused");
            assert!(error.contains("not a github.com address"), "{error}");
        }
    }

    #[test]
    fn a_url_is_not_a_repository_of_ones_own() {
        let error = parse("https://github.com/user/repo", Some("fredrir"))
            .expect_err("the flag is refused");
        assert!(error.contains("not a URL"), "{error}");
    }

    #[test]
    fn a_target_without_a_repository_says_so() {
        for (input, owner) in [("user", None), ("", None), ("", Some("fredrir"))] {
            let error = parse(input, owner).expect_err("the target is refused");
            assert!(error.contains("expected"), "{error}");
        }
    }

    #[test]
    fn a_path_cannot_climb_out_of_the_repository() {
        let error = parse("user/repo/../../etc", None).expect_err("the path is refused");
        assert!(error.contains(".."), "{error}");
    }
}
