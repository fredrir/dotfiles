pub mod avatar;
pub(crate) mod github;
pub(crate) mod subject;

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
            Some("S") => ("×", "Unverified"),
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

pub fn compact_relative_time(timestamp: i64, now: i64) -> String {
    let seconds = now.abs_diff(timestamp);
    let duration = if seconds >= 31_536_000 {
        let count = seconds / 31_536_000;
        format!("{count} year{}", if count == 1 { "" } else { "s" })
    } else if seconds >= 2_592_000 {
        let count = seconds / 2_592_000;
        format!("{count} month{}", if count == 1 { "" } else { "s" })
    } else if seconds >= 86_400 {
        format!("{}d", seconds / 86_400)
    } else if seconds >= 3_600 {
        format!("{}h", seconds / 3_600)
    } else if seconds >= 60 {
        format!("{}min", seconds / 60)
    } else {
        format!("{seconds}s")
    };
    if timestamp > now {
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
