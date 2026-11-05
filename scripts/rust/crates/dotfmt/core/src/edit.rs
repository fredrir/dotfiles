
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Applicability {
    Safe,
    Unsafe,
    Suggestion,
}

impl Applicability {
    pub fn is_safe(self) -> bool {
        self == Self::Safe
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixMode {
    #[default]
    None,
    Safe,
    Unsafe,
}

impl FixMode {
    pub fn allows(self, applicability: Applicability) -> bool {
        match self {
            Self::None => false,
            Self::Safe => applicability == Applicability::Safe,
            Self::Unsafe => matches!(applicability, Applicability::Safe | Applicability::Unsafe),
        }
    }
}

pub trait FixSelection {
    fn selects(&self, fix: &Fix) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tag {
    Unnecessary,
    Deprecated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Related {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub code: String,
    pub message: String,
    pub severity: Severity,
    pub start: usize,
    pub end: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
}

impl Finding {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        severity: Severity,
        start: usize,
        end: usize,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            severity,
            start,
            end,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    pub fn with_tag(mut self, tag: Tag) -> Self {
        self.tags.push(tag);
        self
    }

    pub fn with_related(mut self, related: Related) -> Self {
        self.related.push(related);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub new_text: String,
}

impl Edit {
    pub fn replace(start: usize, end: usize, new_text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            new_text: new_text.into(),
        }
    }

    pub fn delete(start: usize, end: usize) -> Self {
        Self::replace(start, end, "")
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::replace(at, at, text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub title: String,
    pub kind: String,
    pub applicability: Applicability,
    pub edits: Vec<Edit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding: Option<Finding>,
}

impl Fix {
    pub fn new(
        title: impl Into<String>,
        kind: impl Into<String>,
        applicability: Applicability,
        edits: Vec<Edit>,
    ) -> Self {
        Self {
            title: title.into(),
            kind: kind.into(),
            applicability,
            edits,
            finding: None,
        }
    }

    pub fn with_finding(mut self, finding: Finding) -> Self {
        self.finding = Some(finding);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
}

impl Analysis {
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty() && self.fixes.is_empty()
    }

    pub fn extend(&mut self, other: Self) {
        self.findings.extend(other.findings);
        self.fixes.extend(other.fixes);
    }

    pub fn fixes_with(&self, applicability: Applicability) -> impl Iterator<Item = &Fix> {
        self.fixes
            .iter()
            .filter(move |fix| fix.applicability == applicability)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    OutOfBounds,
    NotCharBoundary,
    Overlap,
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OutOfBounds => "edit is outside the source",
            Self::NotCharBoundary => "edit does not fall on a character boundary",
            Self::Overlap => "edits overlap",
        })
    }
}

impl std::error::Error for EditError {}

pub fn apply(source: &str, edits: &[Edit]) -> Result<String, EditError> {
    let mut ordered = edits.to_vec();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    for edit in &ordered {
        if edit.start > edit.end || edit.end > source.len() {
            return Err(EditError::OutOfBounds);
        }
        if !source.is_char_boundary(edit.start) || !source.is_char_boundary(edit.end) {
            return Err(EditError::NotCharBoundary);
        }
    }
    for pair in ordered.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(EditError::Overlap);
        }
    }
    let mut output = source.to_owned();
    for edit in ordered.iter().rev() {
        output.replace_range(edit.start..edit.end, &edit.new_text);
    }
    Ok(output)
}

pub fn select_fixes<'a, S: FixSelection + ?Sized>(
    mode: FixMode,
    selection: &S,
    fixes: &'a [Fix],
) -> Vec<&'a Fix> {
    fixes
        .iter()
        .filter(|fix| mode.allows(fix.applicability) && selection.selects(fix))
        .collect()
}

pub fn apply_fixes(source: &str, fixes: &[&Fix]) -> Result<String, EditError> {
    let edits: Vec<Edit> = fixes
        .iter()
        .flat_map(|fix| fix.edits.iter().cloned())
        .collect();
    apply(source, &edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_edits_backwards_and_allows_adjacent_edits() {
        let source = "abcdef";
        let edits = [
            Edit::replace(0, 1, "A"),
            Edit::replace(2, 3, "C"),
            Edit::insert(3, "!"),
        ];
        assert_eq!(apply(source, &edits).unwrap(), "AbC!def");
    }

    #[test]
    fn rejects_overlaps_and_invalid_boundaries() {
        let source = "abc";
        assert_eq!(
            apply(
                source,
                &[Edit::replace(0, 2, "x"), Edit::replace(1, 3, "y")]
            ),
            Err(EditError::Overlap)
        );
        assert_eq!(
            apply(source, &[Edit::replace(0, 4, "x")]),
            Err(EditError::OutOfBounds)
        );
        assert_eq!(
            apply("é", &[Edit::replace(0, 1, "x")]),
            Err(EditError::NotCharBoundary)
        );
    }

    struct Categories {
        safe_category: bool,
        unsafe_category: bool,
    }

    impl FixSelection for Categories {
        fn selects(&self, fix: &Fix) -> bool {
            if fix.applicability == Applicability::Safe {
                self.safe_category
            } else {
                self.unsafe_category
            }
        }
    }

    #[test]
    fn select_fixes_intersects_applicability_and_categories() {
        let fixes = [
            Fix::new("safe", "quickfix", Applicability::Safe, Vec::new()),
            Fix::new("unsafe", "quickfix", Applicability::Unsafe, Vec::new()),
            Fix::new(
                "suggestion",
                "quickfix",
                Applicability::Suggestion,
                Vec::new(),
            ),
        ];
        let none = Categories {
            safe_category: false,
            unsafe_category: false,
        };
        let safe = Categories {
            safe_category: true,
            unsafe_category: false,
        };
        let all = Categories {
            safe_category: true,
            unsafe_category: true,
        };
        assert!(select_fixes(FixMode::None, &all, &fixes).is_empty());
        assert_eq!(
            select_fixes(FixMode::Safe, &all, &fixes)
                .iter()
                .map(|fix| fix.title.as_str())
                .collect::<Vec<_>>(),
            ["safe"]
        );
        assert_eq!(
            select_fixes(FixMode::Unsafe, &all, &fixes)
                .iter()
                .map(|fix| fix.title.as_str())
                .collect::<Vec<_>>(),
            ["safe", "unsafe"]
        );
        assert!(select_fixes(FixMode::Unsafe, &none, &fixes).is_empty());
        assert_eq!(
            select_fixes(FixMode::Safe, &safe, &fixes)
                .iter()
                .map(|fix| fix.title.as_str())
                .collect::<Vec<_>>(),
            ["safe"]
        );
    }

    #[test]
    fn applies_multiple_fixes() {
        let source = "let a = 1; let b = 2;";
        let one = Fix::new(
            "remove a",
            "quickfix",
            Applicability::Safe,
            vec![Edit::delete(0, 10)],
        );
        let two = Fix::new(
            "remove b",
            "quickfix",
            Applicability::Safe,
            vec![Edit::delete(11, 21)],
        );
        assert_eq!(apply_fixes(source, &[&one, &two]).unwrap(), " ");
    }
}
