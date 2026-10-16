pub mod avatar;
pub(crate) mod github;

use chrono::{DateTime, Utc};

#[derive(Clone, Debug, Default)]
pub struct CommitDetails {
    pub oid: String,
    pub author: String,
    pub author_email: String,
    pub timestamp: i64,
    pub authored_iso: String,
    pub signature: Option<String>,
    pub subject: String,
    pub message: Option<String>,
    pub github: Option<GitHubCommit>,
    pub github_signature_verified: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct GitHubCommit {
    pub url: String,
}

impl CommitDetails {
    pub fn pending(commit: &crate::git::Commit) -> Self {
        Self {
            oid: commit.oid.clone(),
            author: commit.author.clone(),
            author_email: commit.author_email.clone(),
            timestamp: commit.timestamp,
            subject: commit.subject.clone(),
            ..Self::default()
        }
    }

    pub fn short_oid(&self) -> String {
        self.oid.chars().take(7).collect()
    }

    pub fn date(&self) -> String {
        DateTime::parse_from_rfc3339(&self.authored_iso)
            .map(|date| date.format("%B %d, %Y").to_string())
            .unwrap_or_else(|_| {
                DateTime::<Utc>::from_timestamp(self.timestamp, 0).map_or_else(
                    || "Unknown date".into(),
                    |date| date.format("%B %d, %Y").to_string(),
                )
            })
    }

    pub fn verification(&self) -> (&'static str, &'static str) {
        match self.signature.as_deref() {
            Some("B") => ("×", "Invalid signature"),
            Some("X" | "Y") => ("×", "Expired signature"),
            Some("R") => ("×", "Revoked key"),
            Some("G") => ("", "Verified"),
            _ if self.github_signature_verified == Some(true) => ("", "Verified"),
            Some("U") => ("×", "Untrusted signature"),
            Some("N") => ("×", "Unsigned"),
            Some(_) => ("×", "Unverified"),
            None => ("…", "Verifying"),
        }
    }
}

pub fn relative_time(timestamp: i64, now: i64) -> String {
    let future = timestamp > now;
    let seconds = now.abs_diff(timestamp);
    let (count, unit) = [
        (31_536_000, "year"),
        (2_592_000, "month"),
        (604_800, "week"),
        (86_400, "day"),
        (3_600, "hour"),
        (60, "minute"),
        (1, "second"),
    ]
    .into_iter()
    .find_map(|(size, unit)| (seconds >= size).then_some((seconds / size, unit)))
    .unwrap_or((0, "second"));
    let duration = format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    if future {
        format!("in {duration}")
    } else {
        format!("{duration} ago")
    }
}

pub(crate) fn github_repository(remote: &str) -> Option<String> {
    let path = remote
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("git://github.com/"))?;
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    let (owner, repo) = path.split_once('/')?;
    let valid = |value: &str| {
        !value.is_empty()
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    };
    (valid(owner) && valid(repo)).then(|| format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_preserve_author_timezone_and_relative_units() {
        let detail = CommitDetails {
            authored_iso: "2026-10-15T23:30:00-07:00".into(),
            ..Default::default()
        };
        assert_eq!(detail.date(), "October 15, 2026");
        for (seconds, expected) in [
            (1, "1 second ago"),
            (120, "2 minutes ago"),
            (18_000, "5 hours ago"),
            (86_400, "1 day ago"),
            (604_800, "1 week ago"),
            (2_592_000, "1 month ago"),
            (63_072_000, "2 years ago"),
        ] {
            assert_eq!(relative_time(0, seconds), expected);
        }
        assert_eq!(relative_time(60, 0), "in 1 minute");
    }

    #[test]
    fn accepts_only_github_repository_remotes() {
        for remote in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(github_repository(remote).as_deref(), Some("owner/repo"));
        }
        for remote in [
            "https://github.com.evil/owner/repo",
            "https://github.com/../repo",
            "https://github.com/owner/repo/extra",
            "file:///repo",
        ] {
            assert!(github_repository(remote).is_none());
        }
    }

    #[test]
    fn verification_never_confuses_unchecked_with_verified() {
        let mut detail = CommitDetails::default();
        assert_eq!(detail.verification().0, "…");
        for signature in ["B", "E", "X", "Y", "R", "N", "U"] {
            detail.signature = Some(signature.into());
            assert_eq!(detail.verification().0, "×");
        }
        detail.github_signature_verified = Some(true);
        assert_eq!(detail.verification().0, "");
        for signature in ["B", "X", "Y", "R"] {
            detail.signature = Some(signature.into());
            assert_eq!(detail.verification().0, "×");
        }
        detail.signature = Some("G".into());
        detail.github_signature_verified = None;
        assert_eq!(detail.verification(), ("", "Verified"));
    }
}
