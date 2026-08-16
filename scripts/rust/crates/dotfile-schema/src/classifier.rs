use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};

use dotfile_source::RepoPath;
use dotfile_syntax::check_ident;
use unicode_normalization::UnicodeNormalization;

use crate::hir::{AttributeKind, GroupDeclaration, HirRoot, HirValueKind, Profiles, ValidatedFile};

/// The eighteen frozen source/peripheral domains in `schemas.json`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Domain {
    Profiles,
    Hosts,
    GroupRootRequirements,
    FacetRequirements,
    OverrideVariant,
    RecipientKeys,
    SecretScanRules,
    BenchmarkBaselines,
    ThemeRoles,
    ThemeFonts,
    ThemeMapCatppuccin,
    ThemeMapEza,
    ThemeMapGtk,
    ThemeMapKde,
    ThemeMapObsidian,
    ThemeProfiles,
    TemplateVariables,
    GeneratedLock,
}

impl Domain {
    pub const ALL: [Self; 18] = [
        Self::Profiles,
        Self::Hosts,
        Self::GroupRootRequirements,
        Self::FacetRequirements,
        Self::OverrideVariant,
        Self::RecipientKeys,
        Self::SecretScanRules,
        Self::BenchmarkBaselines,
        Self::ThemeRoles,
        Self::ThemeFonts,
        Self::ThemeMapCatppuccin,
        Self::ThemeMapEza,
        Self::ThemeMapGtk,
        Self::ThemeMapKde,
        Self::ThemeMapObsidian,
        Self::ThemeProfiles,
        Self::TemplateVariables,
        Self::GeneratedLock,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Profiles => "profiles",
            Self::Hosts => "hosts",
            Self::GroupRootRequirements => "group_root_requirements",
            Self::FacetRequirements => "facet_requirements",
            Self::OverrideVariant => "override_variant",
            Self::RecipientKeys => "recipient_keys",
            Self::SecretScanRules => "secret_scan_rules",
            Self::BenchmarkBaselines => "benchmark_baselines",
            Self::ThemeRoles => "theme_roles",
            Self::ThemeFonts => "theme_fonts",
            Self::ThemeMapCatppuccin => "theme_map_catppuccin",
            Self::ThemeMapEza => "theme_map_eza",
            Self::ThemeMapGtk => "theme_map_gtk",
            Self::ThemeMapKde => "theme_map_kde",
            Self::ThemeMapObsidian => "theme_map_obsidian",
            Self::ThemeProfiles => "theme_profiles",
            Self::TemplateVariables => "template_variables",
            Self::GeneratedLock => "generated_lock",
        }
    }
}

impl Display for Domain {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Path-derived identity retained alongside a domain classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainLocation {
    Fixed,
    ThemeProfile {
        theme: String,
    },
    GroupRoot {
        group: String,
        directory: RepoPath,
    },
    Facet {
        group: String,
        directory: RepoPath,
        package: String,
    },
    OverrideVariant {
        group: String,
        directory: RepoPath,
        variant: String,
        package: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassifiedPath {
    pub domain: Domain,
    pub location: DomainLocation,
}

/// Complete result for a repository path.  Unknown repository-owned
/// `.dotfile` paths are kept distinct from unrelated payload files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathClassification {
    Known(ClassifiedPath),
    UnknownDotfile,
    NotDotfile,
}

impl PathClassification {
    pub fn known(&self) -> Option<&ClassifiedPath> {
        match self {
            Self::Known(classified) => Some(classified),
            Self::UnknownDotfile | Self::NotDotfile => None,
        }
    }
}

/// One validated directory-bearing group supplied by the profiles domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupLayoutEntry {
    pub group: String,
    pub directory: RepoPath,
}

/// An ambiguity-free group directory map accepted by the dynamic classifier.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GroupLayout {
    entries: Vec<GroupLayoutEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClassificationError {
    InvalidGroupName(String),
    NonCanonicalDirectory(String),
    DuplicateGroup(String),
    DuplicateDirectory(String),
    OverlappingDirectories {
        ancestor: String,
        descendant: String,
    },
    PoisonedProfiles,
}

impl Display for ClassificationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGroupName(name) => write!(formatter, "invalid group name `{name}`"),
            Self::NonCanonicalDirectory(path) => {
                write!(formatter, "group directory is not NFC: `{path}`")
            }
            Self::DuplicateGroup(name) => write!(formatter, "duplicate group `{name}`"),
            Self::DuplicateDirectory(path) => {
                write!(formatter, "duplicate group directory `{path}`")
            }
            Self::OverlappingDirectories {
                ancestor,
                descendant,
            } => write!(
                formatter,
                "group directories overlap: `{ancestor}` contains `{descendant}`"
            ),
            Self::PoisonedProfiles => formatter
                .write_str("profiles HIR is poisoned and cannot supply a classification layout"),
        }
    }
}

impl std::error::Error for ClassificationError {}

impl GroupLayout {
    /// Validates names, NFC, uniqueness, and component-prefix disjointness.
    /// The constructor is intentionally the only way to obtain a layout used
    /// by [`DomainClassifier`].
    pub fn try_new(
        entries: impl IntoIterator<Item = GroupLayoutEntry>,
    ) -> Result<Self, ClassificationError> {
        let mut by_group = BTreeSet::new();
        let mut by_directory = BTreeMap::<String, GroupLayoutEntry>::new();
        for entry in entries {
            if !is_ident_component(&entry.group) {
                return Err(ClassificationError::InvalidGroupName(entry.group));
            }
            if !is_nfc(entry.directory.as_str()) {
                return Err(ClassificationError::NonCanonicalDirectory(
                    entry.directory.as_str().to_owned(),
                ));
            }
            if !by_group.insert(entry.group.clone()) {
                return Err(ClassificationError::DuplicateGroup(entry.group));
            }
            if by_directory
                .insert(entry.directory.as_str().to_owned(), entry.clone())
                .is_some()
            {
                return Err(ClassificationError::DuplicateDirectory(
                    entry.directory.as_str().to_owned(),
                ));
            }
        }

        let entries: Vec<_> = by_directory.into_values().collect();
        for (index, left) in entries.iter().enumerate() {
            for right in entries.iter().skip(index + 1) {
                let (shorter, longer) = if component_count(left.directory.as_str())
                    <= component_count(right.directory.as_str())
                {
                    (left, right)
                } else {
                    (right, left)
                };
                if strip_component_prefix(longer.directory.as_str(), shorter.directory.as_str())
                    .is_some()
                {
                    return Err(ClassificationError::OverlappingDirectories {
                        ancestor: shorter.directory.as_str().to_owned(),
                        descendant: longer.directory.as_str().to_owned(),
                    });
                }
            }
        }

        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[GroupLayoutEntry] {
        &self.entries
    }

    /// Builds the dynamic map only from a sealed, schema-valid profiles file.
    /// Abstract groups without `@directory` are intentionally omitted.
    pub fn from_profiles(validated: &ValidatedFile) -> Result<Self, ClassificationError> {
        let HirRoot::Profiles(profiles) = &validated.hir().root else {
            return Err(ClassificationError::PoisonedProfiles);
        };
        Self::from_profile_hir(profiles)
    }

    fn from_profile_hir(profiles: &Profiles) -> Result<Self, ClassificationError> {
        if profiles_are_poisoned(profiles) {
            return Err(ClassificationError::PoisonedProfiles);
        }
        let mut entries = Vec::new();
        collect_group_layout(&profiles.groups, &mut entries)?;
        Self::try_new(entries)
    }
}

fn profiles_are_poisoned(profiles: &Profiles) -> bool {
    !profiles.poison.is_empty()
        || profiles.version.as_ref().is_none_or(attribute_is_poisoned)
        || profiles.theme.as_ref().is_some_and(attribute_is_poisoned)
        || profiles.profiles.iter().any(|profile| {
            !profile.poison.is_empty() || profile.attributes.iter().any(attribute_is_poisoned)
        })
        || !profiles.groups.iter().any(|group| {
            group.name == "shared"
                && group.parent.is_none()
                && group.attribute(AttributeKind::Directory).is_some()
        })
}

fn attribute_is_poisoned(attribute: &crate::hir::Attribute) -> bool {
    !attribute.poison.is_empty() || !attribute.value.poison.is_empty()
}

fn collect_group_layout(
    groups: &[GroupDeclaration],
    output: &mut Vec<GroupLayoutEntry>,
) -> Result<(), ClassificationError> {
    for group in groups {
        if !group.poison.is_empty() || group.attributes.iter().any(attribute_is_poisoned) {
            return Err(ClassificationError::PoisonedProfiles);
        }
        if let Some(attribute) = group.attribute(AttributeKind::Directory) {
            if !attribute.poison.is_empty() {
                return Err(ClassificationError::PoisonedProfiles);
            }
            let HirValueKind::String(expression) = &attribute.value.kind else {
                return Err(ClassificationError::PoisonedProfiles);
            };
            if expression.evaluated.poisoned {
                return Err(ClassificationError::PoisonedProfiles);
            }
            let directory = RepoPath::new(&expression.evaluated.value)
                .map_err(|_| ClassificationError::PoisonedProfiles)?;
            output.push(GroupLayoutEntry {
                group: group.name.clone(),
                directory,
            });
        }
        collect_group_layout(&group.children, output)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct DomainClassifier {
    layout: GroupLayout,
}

impl DomainClassifier {
    pub fn new(layout: GroupLayout) -> Self {
        Self { layout }
    }

    pub fn without_groups() -> Self {
        Self::default()
    }

    pub fn layout(&self) -> &GroupLayout {
        &self.layout
    }

    pub fn classify(&self, path: &RepoPath) -> PathClassification {
        if let PathClassification::Known(classified) = classify_static(path) {
            return PathClassification::Known(classified);
        }

        if is_nfc(path.as_str()) {
            for entry in self.layout.entries() {
                if let Some(remainder) =
                    strip_component_prefix(path.as_str(), entry.directory.as_str())
                    && let Some(location) = classify_group_remainder(entry, remainder)
                {
                    let domain = match location {
                        DomainLocation::GroupRoot { .. } => Domain::GroupRootRequirements,
                        DomainLocation::Facet { .. } => Domain::FacetRequirements,
                        DomainLocation::OverrideVariant { .. } => Domain::OverrideVariant,
                        DomainLocation::Fixed | DomainLocation::ThemeProfile { .. } => {
                            unreachable!("group classifier produced a static location")
                        }
                    };
                    return PathClassification::Known(ClassifiedPath { domain, location });
                }
            }
        }

        unknown_or_payload(path)
    }
}

/// Classifies every exact or immediate-child domain that does not depend on
/// the profiles file's group directory map.
pub fn classify_static(path: &RepoPath) -> PathClassification {
    let domain = match path.as_str() {
        "config/profiles.dotfile" => Some(Domain::Profiles),
        "config/hosts.dotfile" => Some(Domain::Hosts),
        "config/keys.dotfile" => Some(Domain::RecipientKeys),
        "config/scan.dotfile" => Some(Domain::SecretScanRules),
        "benchmarks/baselines.dotfile" => Some(Domain::BenchmarkBaselines),
        "theme/roles.dotfile" => Some(Domain::ThemeRoles),
        "theme/fonts.dotfile" => Some(Domain::ThemeFonts),
        "theme/maps/catppuccin.dotfile" => Some(Domain::ThemeMapCatppuccin),
        "theme/maps/eza.dotfile" => Some(Domain::ThemeMapEza),
        "theme/maps/gtk.dotfile" => Some(Domain::ThemeMapGtk),
        "theme/maps/kde.dotfile" => Some(Domain::ThemeMapKde),
        "theme/maps/obsidian.dotfile" => Some(Domain::ThemeMapObsidian),
        "vars.enc.yaml" => Some(Domain::TemplateVariables),
        "package.lock.dotfile" => Some(Domain::GeneratedLock),
        _ => None,
    };
    if let Some(domain) = domain {
        return PathClassification::Known(ClassifiedPath {
            domain,
            location: DomainLocation::Fixed,
        });
    }

    if let Some(theme) = immediate_dotfile_child(path.as_str(), "theme/profiles/")
        && is_nfc(theme)
        && is_ident_component(theme)
    {
        return PathClassification::Known(ClassifiedPath {
            domain: Domain::ThemeProfiles,
            location: DomainLocation::ThemeProfile {
                theme: theme.to_owned(),
            },
        });
    }

    unknown_or_payload(path)
}

fn classify_group_remainder(entry: &GroupLayoutEntry, remainder: &str) -> Option<DomainLocation> {
    if remainder == "package.dotfile" {
        return Some(DomainLocation::GroupRoot {
            group: entry.group.clone(),
            directory: entry.directory.clone(),
        });
    }
    let components: Vec<_> = remainder.split('/').collect();
    match components.as_slice() {
        [package, "package.dotfile"] if *package != "overrides" && is_ident_component(package) => {
            Some(DomainLocation::Facet {
                group: entry.group.clone(),
                directory: entry.directory.clone(),
                package: (*package).to_owned(),
            })
        }
        ["overrides", variant, package, "package.dotfile"]
            if is_ident_component(variant)
                && is_ident_component(package)
                && !matches!(*variant, "base" | "none") =>
        {
            Some(DomainLocation::OverrideVariant {
                group: entry.group.clone(),
                directory: entry.directory.clone(),
                variant: (*variant).to_owned(),
                package: (*package).to_owned(),
            })
        }
        _ => None,
    }
}

fn immediate_dotfile_child<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let child = path.strip_prefix(prefix)?;
    if child.contains('/') {
        return None;
    }
    child
        .strip_suffix(".dotfile")
        .filter(|name| !name.is_empty())
}

fn unknown_or_payload(path: &RepoPath) -> PathClassification {
    if path.as_str().ends_with(".dotfile") {
        PathClassification::UnknownDotfile
    } else {
        PathClassification::NotDotfile
    }
}

fn component_count(path: &str) -> usize {
    path.split('/').count()
}

/// Returns the suffix after an exact component prefix, or `None` when the
/// strings only share a byte prefix (`foo` versus `foobar`).
fn strip_component_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path == prefix {
        return Some("");
    }
    path.strip_prefix(prefix)?.strip_prefix('/')
}

fn is_nfc(value: &str) -> bool {
    value.nfc().eq(value.chars())
}

fn is_ident_component(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first.is_ascii_digit() || first == b'.')
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
        && check_ident(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(path: &str) -> RepoPath {
        RepoPath::new(path).unwrap()
    }

    fn known(value: &str) -> ClassifiedPath {
        let PathClassification::Known(classified) = classify_static(&path(value)) else {
            panic!("expected known path: {value}");
        };
        classified
    }

    #[test]
    fn every_frozen_domain_is_representable() {
        let representatives = [
            ("config/profiles.dotfile", Domain::Profiles),
            ("config/hosts.dotfile", Domain::Hosts),
            ("config/keys.dotfile", Domain::RecipientKeys),
            ("config/scan.dotfile", Domain::SecretScanRules),
            ("benchmarks/baselines.dotfile", Domain::BenchmarkBaselines),
            ("theme/roles.dotfile", Domain::ThemeRoles),
            ("theme/fonts.dotfile", Domain::ThemeFonts),
            ("theme/maps/catppuccin.dotfile", Domain::ThemeMapCatppuccin),
            ("theme/maps/eza.dotfile", Domain::ThemeMapEza),
            ("theme/maps/gtk.dotfile", Domain::ThemeMapGtk),
            ("theme/maps/kde.dotfile", Domain::ThemeMapKde),
            ("theme/maps/obsidian.dotfile", Domain::ThemeMapObsidian),
            ("theme/profiles/mocha.dotfile", Domain::ThemeProfiles),
            ("vars.enc.yaml", Domain::TemplateVariables),
            ("package.lock.dotfile", Domain::GeneratedLock),
        ];
        for (path, domain) in representatives {
            assert_eq!(known(path).domain, domain, "{path}");
        }

        let layout = GroupLayout::try_new([GroupLayoutEntry {
            group: "shared".into(),
            directory: path("shared"),
        }])
        .unwrap();
        let classifier = DomainClassifier::new(layout);
        let dynamic = [
            ("shared/package.dotfile", Domain::GroupRootRequirements),
            ("shared/zsh/package.dotfile", Domain::FacetRequirements),
            (
                "shared/overrides/laptop/zsh/package.dotfile",
                Domain::OverrideVariant,
            ),
        ];
        for (path, domain) in dynamic {
            let PathClassification::Known(classified) = classifier.classify(&self::path(path))
            else {
                panic!("expected dynamic path: {path}");
            };
            assert_eq!(classified.domain, domain);
        }
        assert_eq!(Domain::ALL.len(), representatives.len() + dynamic.len());
    }

    #[test]
    fn dynamic_layout_is_component_safe_and_validated() {
        let overlap = GroupLayout::try_new([
            GroupLayoutEntry {
                group: "linux".into(),
                directory: path("linux"),
            },
            GroupLayoutEntry {
                group: "arch".into(),
                directory: path("linux/arch"),
            },
        ]);
        assert!(matches!(
            overlap,
            Err(ClassificationError::OverlappingDirectories { .. })
        ));

        let classifier = DomainClassifier::new(
            GroupLayout::try_new([GroupLayoutEntry {
                group: "foo".into(),
                directory: path("foo"),
            }])
            .unwrap(),
        );
        assert_eq!(
            classifier.classify(&path("foobar/x/package.dotfile")),
            PathClassification::UnknownDotfile
        );
        assert_eq!(
            classifier.classify(&path("foo/overrides/none/x/package.dotfile")),
            PathClassification::UnknownDotfile
        );
    }

    #[test]
    fn immediate_theme_children_only() {
        assert_eq!(
            known("theme/profiles/7-dark.dotfile").domain,
            Domain::ThemeProfiles
        );
        assert_eq!(
            classify_static(&path("theme/profiles/nested/dark.dotfile")),
            PathClassification::UnknownDotfile
        );
        assert_eq!(
            classify_static(&path("other/readme.txt")),
            PathClassification::NotDotfile
        );
        for invalid in ["a b", "a@b", "a💩", "_private"] {
            assert_eq!(
                classify_static(&path(&format!("theme/profiles/{invalid}.dotfile"))),
                PathClassification::UnknownDotfile
            );
        }
    }
}
