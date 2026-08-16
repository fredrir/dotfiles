use std::fmt::{self, Display, Formatter};

/// A strict repository-relative path.
///
/// Validation covers the structural rules of the language: UTF-8, `/`
/// separators, no empty, `.` or `..` component, no backslash, no control
/// character, and no leading or trailing slash. NFC equivalence checking is
/// layered on top of this type by the schema and discovery stages, which own
/// the normalization policy.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepoPath(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathError {
    Empty,
    LeadingSlash,
    TrailingSlash,
    EmptyComponent,
    DotComponent,
    DotDotComponent,
    Backslash,
    Control,
}

impl Display for PathError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "path is empty",
            Self::LeadingSlash => "path has a leading slash",
            Self::TrailingSlash => "path has a trailing slash",
            Self::EmptyComponent => "path has an empty component",
            Self::DotComponent => "path has a `.` component",
            Self::DotDotComponent => "path has a `..` component",
            Self::Backslash => "path contains a backslash",
            Self::Control => "path contains a control character",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PathError {}

impl RepoPath {
    pub fn new(path: &str) -> Result<Self, PathError> {
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        if path.starts_with('/') {
            return Err(PathError::LeadingSlash);
        }
        if path.ends_with('/') {
            return Err(PathError::TrailingSlash);
        }
        for component in path.split('/') {
            if component.is_empty() {
                return Err(PathError::EmptyComponent);
            }
            if component == "." {
                return Err(PathError::DotComponent);
            }
            if component == ".." {
                return Err(PathError::DotDotComponent);
            }
        }
        for scalar in path.chars() {
            if scalar == '\\' {
                return Err(PathError::Backslash);
            }
            if scalar.is_control() {
                return Err(PathError::Control);
            }
        }
        Ok(Self(path.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for RepoPath {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_strict_relative_paths() {
        assert!(RepoPath::new("config/profiles.dotfile").is_ok());
        assert!(RepoPath::new("shared/wezterm/types").is_ok());
        assert!(RepoPath::new("foo..bar").is_ok());
        assert!(RepoPath::new("a/%=@+-.b").is_ok());
    }

    #[test]
    fn rejects_unsafe_paths() {
        assert_eq!(RepoPath::new(""), Err(PathError::Empty));
        assert_eq!(RepoPath::new("/etc"), Err(PathError::LeadingSlash));
        assert_eq!(RepoPath::new("a/"), Err(PathError::TrailingSlash));
        assert_eq!(RepoPath::new("a//b"), Err(PathError::EmptyComponent));
        assert_eq!(RepoPath::new("./a"), Err(PathError::DotComponent));
        assert_eq!(RepoPath::new("a/../b"), Err(PathError::DotDotComponent));
        assert_eq!(RepoPath::new("a\\b"), Err(PathError::Backslash));
        assert_eq!(RepoPath::new("a\u{7}b"), Err(PathError::Control));
    }
}
