//! Frozen, path-selected ordering metadata for the v1 formatter.
//!
//! This module deliberately contains data rather than formatter policy.  A
//! formatter can therefore stay total over schema-invalid input while using
//! the same published ordering tables as validation and generated writers.

use crate::Domain;

/// The semantic attribute context used by requirement-style containers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FormatContext {
    Fact,
    Deployment,
    Group,
    Profile,
    Host,
    GroupRoot,
}

/// A structural category whose legality affects requirement-block sorting.
///
/// An illegal category is not discarded; the formatter treats it as a stable
/// barrier so schema-invalid input remains lossless and deterministic.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FormatEntryKind {
    Path,
    Resource,
    Extension,
    Demand,
    ResourceIdentity,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FormatPublishedKind {
    Attribute,
    NamedValue,
    NamedBlock,
    SigilBlock,
}

/// How the direct entries of one container are ordered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatOrder {
    /// Entry order is semantic and must not be changed.
    Preserve,
    /// Named entries sort by ascending unsigned UTF-8 bytes.
    NamesByBytes,
    /// Apply the generic requirement-block category ordering, using the
    /// supplied context for attributes within the attribute category.
    Requirement(FormatContext),
    /// Move known singleton fields/blocks to their published positions.
    /// Unknown and duplicate entries remain formatter barriers.
    Published(&'static [&'static str]),
    /// Collect direct open-map scalar entries into one leading category while
    /// retaining their source order, followed by known structural entries in
    /// their published order. Unknown structural entries and other entry kinds
    /// remain formatter barriers.
    OpenThenPublished(&'static [&'static str]),
}

/// An ordering override for a container below a schema root.
///
/// Paths use semantic entry names.  `"*"` matches exactly one dynamic named
/// component (for example a host name or a group declaration).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FormatContainerRule {
    pub path: &'static [&'static str],
    pub order: FormatOrder,
}

/// All formatter ordering information selected by one classified domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FormatSchema {
    pub domain: Domain,
    pub root_order: FormatOrder,
    pub container_rules: &'static [FormatContainerRule],
}

impl FormatSchema {
    /// Returns the most-specific exact-length rule for `path`.
    ///
    /// Literal components are more specific than wildcard components.  An
    /// unregistered container is ordered conservatively in source order.
    pub fn order_for(&self, path: &[&str]) -> FormatOrder {
        if path.is_empty() {
            return self.root_order;
        }

        self.container_rules
            .iter()
            .filter_map(|rule| {
                path_match_specificity(rule.path, path).map(|specificity| (specificity, rule.order))
            })
            .max_by_key(|(specificity, _)| *specificity)
            .map_or(FormatOrder::Preserve, |(_, order)| order)
    }

    /// Convenience form for consumers that keep the lookup next to a schema.
    pub fn attribute_order(context: FormatContext, name: &str) -> Option<u16> {
        attribute_order(context, name)
    }

    /// Returns a published position only when the attribute is legal at the
    /// exact domain/container path. Known-but-misplaced attributes therefore
    /// remain stable formatter barriers.
    pub fn attribute_order_for(
        &self,
        path: &[&str],
        context: FormatContext,
        name: &str,
    ) -> Option<u16> {
        attribute_allowed(self.domain, path, name).then(|| attribute_order(context, name))?
    }

    /// Registered resource-kind order. Unknown sigil blocks are barriers.
    pub fn resource_kind_order(&self, name: &str) -> Option<u16> {
        (name == "font").then_some(10)
    }

    /// Expected generic-syntax shape for a published field or structural
    /// block. Dynamic open-map entries return a shape where the domain owns
    /// one; unrelated/unknown names return `None`.
    pub fn expected_entry_shape(&self, path: &[&str], name: &str) -> Option<FormatPublishedKind> {
        expected_entry_shape(self.domain, path, name)
    }

    pub fn published_entry_allowed(
        &self,
        path: &[&str],
        name: &str,
        kind: FormatPublishedKind,
        optional: bool,
    ) -> bool {
        !optional && self.expected_entry_shape(path, name) == Some(kind)
    }

    /// Whether a structural category is legal at a requirement-domain path.
    ///
    /// This is deliberately separate from [`FormatOrder`]: the ordering
    /// context describes known attributes, while the domain and semantic path
    /// decide which nested syntax categories can safely participate in a sort.
    pub fn entry_allowed(&self, path: &[&str], kind: FormatEntryKind) -> bool {
        match self.domain {
            Domain::GroupRootRequirements | Domain::FacetRequirements => {
                if let Some(container) = path.last().copied() {
                    return match container {
                        // Entity bodies contain facts plus nested entity demands.
                        "entity_demand" => {
                            matches!(kind, FormatEntryKind::Demand | FormatEntryKind::Resource)
                        }
                        // Resource identity is legal only as a direct resource field.
                        "resource_demand" => kind == FormatEntryKind::ResourceIdentity,
                        // Extension and path bodies contain bindings and attributes.
                        "@extend" | "path" => false,
                        _ => false,
                    };
                }

                match self.domain {
                    Domain::GroupRootRequirements => matches!(
                        kind,
                        FormatEntryKind::Resource
                            | FormatEntryKind::Extension
                            | FormatEntryKind::Demand
                    ),
                    Domain::FacetRequirements => matches!(
                        kind,
                        FormatEntryKind::Path
                            | FormatEntryKind::Resource
                            | FormatEntryKind::Extension
                            | FormatEntryKind::Demand
                    ),
                    _ => unreachable!("outer match restricts requirement domains"),
                }
            }
            Domain::OverrideVariant => path.is_empty() && kind == FormatEntryKind::Path,
            Domain::Profiles
            | Domain::Hosts
            | Domain::RecipientKeys
            | Domain::SecretScanRules
            | Domain::BenchmarkBaselines
            | Domain::ThemeRoles
            | Domain::ThemeFonts
            | Domain::ThemeMapCatppuccin
            | Domain::ThemeMapEza
            | Domain::ThemeMapGtk
            | Domain::ThemeMapKde
            | Domain::ThemeMapObsidian
            | Domain::ThemeProfiles
            | Domain::TemplateVariables
            | Domain::GeneratedLock => false,
        }
    }
}

fn path_match_specificity(rule: &[&str], actual: &[&str]) -> Option<usize> {
    if rule.len() != actual.len() {
        return None;
    }

    let mut literal_components = 0;
    for (expected, found) in rule.iter().zip(actual) {
        if *expected == "*" {
            continue;
        }
        if expected != found {
            return None;
        }
        literal_components += 1;
    }
    Some(literal_components)
}

/// Returns the v1 published position of a known attribute/field.
///
/// `hostnames` and `role` are ordinary assignments, but are included in the
/// host context because the host schema publishes one combined field order.
pub fn attribute_order(context: FormatContext, name: &str) -> Option<u16> {
    match context {
        FormatContext::Fact => match name {
            "@pkg" => Some(100),
            "@installer" => Some(110),
            "@bin" => Some(120),
            "@check" => Some(130),
            "@version" => Some(140),
            "@family" => Some(150),
            "@service" => Some(160),
            "@scope" => Some(170),
            "@path" => Some(180),
            "@description" => Some(190),
            _ => None,
        },
        FormatContext::Deployment => match name {
            "@destination" => Some(100),
            "@deploy" => Some(110),
            "@privilege" => Some(120),
            "@sensitivity" => Some(130),
            "@mode" => Some(140),
            "@owner" => Some(150),
            "@group" => Some(160),
            "@expect" => Some(170),
            "@description" => Some(180),
            "@theme" => Some(190),
            _ => None,
        },
        FormatContext::Group => match name {
            "@directory" => Some(100),
            "@os" => Some(110),
            "@arch" => Some(120),
            "@description" => Some(130),
            _ => None,
        },
        FormatContext::Profile => match name {
            "@groups" => Some(100),
            "@manager" => Some(110),
            "@os" => Some(120),
            "@arch" => Some(130),
            "@theme" => Some(140),
            "@description" => Some(150),
            _ => None,
        },
        FormatContext::Host => match name {
            "hostnames" => Some(80),
            "role" => Some(90),
            "@profile" => Some(100),
            "@theme" => Some(110),
            _ => None,
        },
        FormatContext::GroupRoot => match name {
            "@theme" => Some(100),
            _ => None,
        },
    }
}

/// Returns the frozen v1 formatter schema for `domain`.
pub const fn format_schema(domain: Domain) -> &'static FormatSchema {
    match domain {
        Domain::Profiles => &PROFILES_SCHEMA,
        Domain::Hosts => &HOSTS_SCHEMA,
        Domain::GroupRootRequirements => &GROUP_ROOT_REQUIREMENTS_SCHEMA,
        Domain::FacetRequirements => &FACET_REQUIREMENTS_SCHEMA,
        Domain::OverrideVariant => &OVERRIDE_VARIANT_SCHEMA,
        Domain::RecipientKeys => &RECIPIENT_KEYS_SCHEMA,
        Domain::SecretScanRules => &SECRET_SCAN_RULES_SCHEMA,
        Domain::BenchmarkBaselines => &BENCHMARK_BASELINES_SCHEMA,
        Domain::ThemeRoles => &THEME_ROLES_SCHEMA,
        Domain::ThemeFonts => &THEME_FONTS_SCHEMA,
        Domain::ThemeMapCatppuccin => &THEME_MAP_CATPPUCCIN_SCHEMA,
        Domain::ThemeMapEza => &THEME_MAP_EZA_SCHEMA,
        Domain::ThemeMapGtk => &THEME_MAP_GTK_SCHEMA,
        Domain::ThemeMapKde => &THEME_MAP_KDE_SCHEMA,
        Domain::ThemeMapObsidian => &THEME_MAP_OBSIDIAN_SCHEMA,
        Domain::ThemeProfiles => &THEME_PROFILE_SCHEMA,
        Domain::TemplateVariables => &TEMPLATE_VARIABLES_SCHEMA,
        Domain::GeneratedLock => &GENERATED_LOCK_SCHEMA,
    }
}

/// Shorthand for callers that do not retain a [`FormatSchema`].
pub fn order_for(domain: Domain, path: &[&str]) -> FormatOrder {
    format_schema(domain).order_for(path)
}

/// Shorthand for callers that do not retain a [`FormatSchema`].
pub fn entry_allowed(domain: Domain, path: &[&str], kind: FormatEntryKind) -> bool {
    format_schema(domain).entry_allowed(path, kind)
}

/// Whether `name` is a legal attribute at one requirement-domain path.
pub fn attribute_allowed(domain: Domain, path: &[&str], name: &str) -> bool {
    if domain == Domain::Profiles {
        if path.len() >= 2 && path.first() == Some(&"@groups") {
            return matches!(name, "@directory" | "@os" | "@arch" | "@description");
        }
        return match path {
            ["@profiles", _] => matches!(
                name,
                "@groups" | "@manager" | "@os" | "@arch" | "@theme" | "@description"
            ),
            [] => matches!(name, "@dotfile-version" | "@theme"),
            _ => false,
        };
    }
    if domain == Domain::Hosts {
        return matches!(path, [_]) && matches!(name, "@profile" | "@theme");
    }
    let entity_fact = matches!(
        name,
        "@pkg"
            | "@installer"
            | "@bin"
            | "@check"
            | "@version"
            | "@family"
            | "@service"
            | "@scope"
            | "@path"
            | "@description"
    );
    let resource_fact = matches!(
        name,
        "@key" | "@pkg" | "@installer" | "@check" | "@version" | "@family" | "@description"
    );
    let facet_deployment = matches!(
        name,
        "@destination"
            | "@deploy"
            | "@privilege"
            | "@sensitivity"
            | "@mode"
            | "@owner"
            | "@group"
            | "@description"
            | "@theme"
    );
    let path_deployment = matches!(
        name,
        "@destination"
            | "@deploy"
            | "@privilege"
            | "@sensitivity"
            | "@mode"
            | "@owner"
            | "@group"
            | "@expect"
    );

    match path.last().copied() {
        Some("entity_demand") | Some("entity") => entity_fact,
        Some("resource_demand") | Some("font") => resource_fact,
        Some("entity_extension") => entity_fact && name != "@key",
        Some("resource_extension") => resource_fact && name != "@key",
        Some("path") => path_deployment,
        Some("@extend") => resource_fact && name != "@key",
        Some(_) => false,
        None => match domain {
            Domain::GroupRootRequirements => name == "@theme",
            Domain::FacetRequirements | Domain::OverrideVariant => facet_deployment,
            Domain::Profiles => false,
            Domain::Hosts => false,
            _ => false,
        },
    }
}

pub fn resource_kind_order(name: &str) -> Option<u16> {
    (name == "font").then_some(10)
}

pub fn expected_entry_shape(
    domain: Domain,
    path: &[&str],
    name: &str,
) -> Option<FormatPublishedKind> {
    use FormatPublishedKind::{Attribute, NamedBlock, NamedValue, SigilBlock};

    if path.is_empty() {
        return match domain {
            Domain::Profiles => match name {
                "@dotfile-version" | "@theme" => Some(Attribute),
                "@groups" | "@profiles" => Some(SigilBlock),
                _ => None,
            },
            Domain::Hosts | Domain::BenchmarkBaselines => Some(NamedBlock),
            Domain::RecipientKeys if name == "recipients" => Some(NamedBlock),
            Domain::SecretScanRules if name == "allow" => Some(NamedBlock),
            Domain::GroupRootRequirements | Domain::FacetRequirements | Domain::OverrideVariant => {
                None
            }
            Domain::ThemeProfiles => match name {
                "display-name" | "appearance" | "icons" => Some(NamedValue),
                "nvim" | "palette" | "roles" | "terminal" | "eza" | "kde" | "konsole" | "fonts"
                | "sizes" | "applications" => Some(NamedBlock),
                _ => None,
            },
            Domain::ThemeRoles
                if matches!(name, "roles" | "terminal" | "eza" | "kde" | "konsole") =>
            {
                Some(NamedBlock)
            }
            Domain::ThemeFonts if matches!(name, "fonts" | "sizes" | "applications") => {
                Some(NamedBlock)
            }
            Domain::ThemeMapCatppuccin if name == "colors" => Some(NamedBlock),
            Domain::ThemeMapEza if name == "categories" => Some(NamedBlock),
            Domain::ThemeMapGtk if name == "colors" => Some(NamedBlock),
            Domain::ThemeMapKde
                if matches!(name, "groups" | "foregrounds" | "selection-foregrounds") =>
            {
                Some(NamedBlock)
            }
            Domain::ThemeMapObsidian if matches!(name, "derived" | "variables") => Some(NamedBlock),
            Domain::GeneratedLock if name.starts_with('@') => Some(SigilBlock),
            Domain::RecipientKeys
            | Domain::SecretScanRules
            | Domain::ThemeRoles
            | Domain::ThemeFonts
            | Domain::ThemeMapCatppuccin
            | Domain::ThemeMapEza
            | Domain::ThemeMapGtk
            | Domain::ThemeMapKde
            | Domain::ThemeMapObsidian
            | Domain::TemplateVariables
            | Domain::GeneratedLock => None,
        };
    }

    match (domain, path) {
        (Domain::Profiles, ["@groups", _])
        | (Domain::Profiles, ["@profiles", _])
        | (Domain::Hosts, [_]) => name
            .starts_with('@')
            .then_some(Attribute)
            .or(Some(NamedValue)),
        (Domain::RecipientKeys, ["recipients"]) | (Domain::BenchmarkBaselines, [_]) => {
            Some(NamedValue)
        }
        (Domain::SecretScanRules, ["allow"]) if name == "rule" => Some(NamedBlock),
        (Domain::SecretScanRules, ["allow", "rule"]) if matches!(name, "pattern" | "inspect") => {
            Some(NamedValue)
        }
        (Domain::ThemeRoles | Domain::ThemeProfiles, ["terminal"])
            if matches!(name, "ansi" | "tabs") =>
        {
            Some(NamedBlock)
        }
        (Domain::ThemeRoles | Domain::ThemeProfiles, ["eza"])
            if matches!(name, "categories" | "pattern") =>
        {
            Some(NamedBlock)
        }
        _ => match format_schema(domain).order_for(path) {
            FormatOrder::Published(fields) | FormatOrder::OpenThenPublished(fields)
                if fields.contains(&name) =>
            {
                Some(NamedValue)
            }
            _ => None,
        },
    }
}

pub fn published_entry_allowed(
    domain: Domain,
    path: &[&str],
    name: &str,
    kind: FormatPublishedKind,
    optional: bool,
) -> bool {
    !optional && expected_entry_shape(domain, path, name) == Some(kind)
}

const PROFILES_ROOT: &[&str] = &["@dotfile-version", "@groups", "@profiles", "@theme"];
const THEME_ROLES_ROOT: &[&str] = &["roles", "terminal", "eza", "kde", "konsole"];
const THEME_FONTS_ROOT: &[&str] = &["fonts", "sizes", "applications"];
const THEME_PROFILE_ROOT: &[&str] = &[
    "display-name",
    "appearance",
    "icons",
    "nvim",
    "palette",
    "roles",
    "terminal",
    "eza",
    "kde",
    "konsole",
    "fonts",
    "sizes",
    "applications",
];
const THEME_MAP_KDE_ROOT: &[&str] = &["groups", "foregrounds", "selection-foregrounds"];
const GENERATED_LOCK_ROOT: &[&str] = &[
    "@lock",
    "@sources",
    "@groups",
    "@profiles",
    "@facets",
    "@nodes",
    "@facts",
    "@occurrences",
    "@paths",
    "@mappings",
    "@effective",
    "@deployments",
    "@themes",
    "@hosts",
    "@defaults",
];

const TERMINAL_TAIL: &[&str] = &["ansi", "tabs"];
const EZA_TAIL: &[&str] = &["categories", "pattern"];
const SIZES: &[&str] = &["terminal", "terminal_mac", "interface"];
const PATTERN_FIELDS: &[&str] = &["key", "role"];
const KEY_PALETTE_FIELDS: &[&str] = &["key", "palette"];
const CATEGORY_FIELDS: &[&str] = &["name", "extensions"];
const KEY_ROLE_FIELDS: &[&str] = &["key", "role"];
const KEY_ROLES_FIELDS: &[&str] = &["key", "roles"];
const OBSIDIAN_VARIABLE_FIELDS: &[&str] = &[
    "key", "palette", "rgb", "color", "alpha", "derived", "literal",
];

const PROFILES_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["@groups"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["@groups", "*"],
        order: FormatOrder::Requirement(FormatContext::Group),
    },
    FormatContainerRule {
        path: &["@profiles"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["@profiles", "*"],
        order: FormatOrder::Requirement(FormatContext::Profile),
    },
];

const HOSTS_RULES: &[FormatContainerRule] = &[FormatContainerRule {
    path: &["*"],
    order: FormatOrder::Requirement(FormatContext::Host),
}];

const GROUP_ROOT_REQUIREMENTS_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["entity_demand"],
        order: FormatOrder::Requirement(FormatContext::Fact),
    },
    FormatContainerRule {
        path: &["resource_demand"],
        order: FormatOrder::Requirement(FormatContext::Fact),
    },
    FormatContainerRule {
        path: &["@extend"],
        order: FormatOrder::Requirement(FormatContext::Fact),
    },
];

const FACET_REQUIREMENTS_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["path"],
        order: FormatOrder::Requirement(FormatContext::Deployment),
    },
    FormatContainerRule {
        path: &["entity_demand"],
        order: FormatOrder::Requirement(FormatContext::Fact),
    },
    FormatContainerRule {
        path: &["resource_demand"],
        order: FormatOrder::Requirement(FormatContext::Fact),
    },
    FormatContainerRule {
        path: &["@extend"],
        order: FormatOrder::Requirement(FormatContext::Fact),
    },
];

const OVERRIDE_VARIANT_RULES: &[FormatContainerRule] = &[FormatContainerRule {
    path: &["path"],
    order: FormatOrder::Requirement(FormatContext::Deployment),
}];

const RECIPIENT_KEYS_RULES: &[FormatContainerRule] = &[FormatContainerRule {
    path: &["recipients"],
    order: FormatOrder::NamesByBytes,
}];

const SECRET_SCAN_RULES_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["allow"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["allow", "rule"],
        order: FormatOrder::Published(&["pattern", "inspect"]),
    },
];

const BENCHMARK_RULES: &[FormatContainerRule] = &[FormatContainerRule {
    path: &["*"],
    order: FormatOrder::NamesByBytes,
}];

const THEME_ROLES_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["roles"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["terminal"],
        order: FormatOrder::OpenThenPublished(TERMINAL_TAIL),
    },
    FormatContainerRule {
        path: &["terminal", "ansi"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["terminal", "tabs"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["eza"],
        order: FormatOrder::OpenThenPublished(EZA_TAIL),
    },
    FormatContainerRule {
        path: &["eza", "categories"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["eza", "pattern"],
        order: FormatOrder::Published(PATTERN_FIELDS),
    },
    FormatContainerRule {
        path: &["kde"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["konsole"],
        order: FormatOrder::Preserve,
    },
];

const THEME_FONTS_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["fonts"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["sizes"],
        order: FormatOrder::Published(SIZES),
    },
    FormatContainerRule {
        path: &["applications"],
        order: FormatOrder::Preserve,
    },
];

const THEME_PROFILE_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["nvim"],
        order: FormatOrder::Published(&["flavour"]),
    },
    FormatContainerRule {
        path: &["palette"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["roles"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["terminal"],
        order: FormatOrder::OpenThenPublished(TERMINAL_TAIL),
    },
    FormatContainerRule {
        path: &["terminal", "ansi"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["terminal", "tabs"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["eza"],
        order: FormatOrder::OpenThenPublished(EZA_TAIL),
    },
    FormatContainerRule {
        path: &["eza", "categories"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["eza", "pattern"],
        order: FormatOrder::Published(PATTERN_FIELDS),
    },
    FormatContainerRule {
        path: &["kde"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["konsole"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["fonts"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["sizes"],
        order: FormatOrder::Published(SIZES),
    },
    FormatContainerRule {
        path: &["applications"],
        order: FormatOrder::Preserve,
    },
];

const THEME_MAP_CATPPUCCIN_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["colors"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["colors", "entry"],
        order: FormatOrder::Published(KEY_PALETTE_FIELDS),
    },
];

const THEME_MAP_EZA_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["categories"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["categories", "category"],
        order: FormatOrder::Published(CATEGORY_FIELDS),
    },
];

const THEME_MAP_GTK_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["colors"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["colors", "entry"],
        order: FormatOrder::Published(KEY_ROLE_FIELDS),
    },
];

const THEME_MAP_KDE_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["groups"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["groups", "entry"],
        order: FormatOrder::Published(KEY_ROLES_FIELDS),
    },
    FormatContainerRule {
        path: &["foregrounds"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["foregrounds", "entry"],
        order: FormatOrder::Published(KEY_ROLE_FIELDS),
    },
    FormatContainerRule {
        path: &["selection-foregrounds"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["selection-foregrounds", "entry"],
        order: FormatOrder::Published(KEY_ROLE_FIELDS),
    },
];

const THEME_MAP_OBSIDIAN_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["derived"],
        order: FormatOrder::Published(&["source"]),
    },
    FormatContainerRule {
        path: &["variables"],
        order: FormatOrder::Preserve,
    },
    FormatContainerRule {
        path: &["variables", "variable"],
        order: FormatOrder::Published(OBSIDIAN_VARIABLE_FIELDS),
    },
];

const LOCK_HEADER_FIELDS: &[&str] = &[
    "dotfile-version",
    "lock-version",
    "builtins-version",
    "ir",
    "structure",
];
const LOCK_SOURCE_FIELDS: &[&str] = &["path", "domain", "hash"];
const LOCK_GROUP_FIELDS: &[&str] = &[
    "id",
    "name",
    "ancestors",
    "parent",
    "directory",
    "os",
    "arch",
    "description",
];
const LOCK_PROFILE_FIELDS: &[&str] = &[
    "id",
    "groups",
    "manager",
    "installer",
    "os",
    "arch",
    "theme",
    "description",
];
const LOCK_FACET_FIELDS: &[&str] = &[
    "id",
    "group",
    "package",
    "directory",
    "variant",
    "source_span",
    "description",
    "theme",
    "destination",
    "deploy",
    "privilege",
    "sensitivity",
    "mode",
    "owner",
    "owner_group",
];
const LOCK_NODE_FIELDS: &[&str] = &["id", "node_kind", "resource_kind", "resource_key"];
const LOCK_FACT_FIELDS: &[&str] = &["target", "attribute", "scope", "value", "source_span"];
const LOCK_OCCURRENCE_FIELDS: &[&str] = &[
    "id",
    "target",
    "root",
    "group",
    "parent",
    "local_mode",
    "effective_mode",
    "source_span",
    "reasons",
];
const LOCK_ASSERTION_FIELDS: &[&str] = &[
    "id",
    "facet",
    "path",
    "demand_mode",
    "expect",
    "source_span",
];
const LOCK_MAPPING_FIELDS: &[&str] = &[
    "facet",
    "source_prefix",
    "deploy",
    "privilege",
    "sensitivity",
    "origin",
    "destination",
    "mode",
    "owner",
    "owner_group",
    "source_span",
];
const LOCK_RESOLUTION_FIELDS: &[&str] = &[
    "profile",
    "target",
    "demand_mode",
    "check",
    "bin",
    "family",
    "service",
    "scope",
    "path",
    "installer",
    "package",
    "version",
    "provenance",
];
const LOCK_CANDIDATE_FIELDS: &[&str] = &[
    "facet",
    "declaration_group",
    "variant",
    "physical_source",
    "logical_source",
    "output_source",
    "destination",
    "action",
    "render",
    "privilege",
    "sensitivity",
    "mode",
    "owner",
    "owner_group",
    "source_type",
    "source_digest",
    "vault_source",
    "vault_digest",
    "provenance",
];
const LOCK_THEME_FIELDS: &[&str] = &["id", "name", "path"];
const LOCK_THEME_CONTRIBUTION_FIELDS: &[&str] = &["group", "theme", "source_span"];
const LOCK_THEME_RESOLUTION_FIELDS: &[&str] = &[
    "profile",
    "group_theme",
    "profile_theme",
    "default_theme",
    "provenance",
];
const LOCK_HOST_FIELDS: &[&str] = &["id", "name", "hostnames", "role", "profile", "theme"];
const LOCK_HOST_FACT_FIELDS: &[&str] = &["host", "key", "value"];
const LOCK_DEFAULT_FIELDS: &[&str] = &["key", "value", "source_span"];

const GENERATED_LOCK_RULES: &[FormatContainerRule] = &[
    FormatContainerRule {
        path: &["@lock"],
        order: FormatOrder::Published(LOCK_HEADER_FIELDS),
    },
    FormatContainerRule {
        path: &["@sources", "source"],
        order: FormatOrder::Published(LOCK_SOURCE_FIELDS),
    },
    FormatContainerRule {
        path: &["@groups", "group"],
        order: FormatOrder::Published(LOCK_GROUP_FIELDS),
    },
    FormatContainerRule {
        path: &["@profiles", "profile"],
        order: FormatOrder::Published(LOCK_PROFILE_FIELDS),
    },
    FormatContainerRule {
        path: &["@facets", "facet"],
        order: FormatOrder::Published(LOCK_FACET_FIELDS),
    },
    FormatContainerRule {
        path: &["@nodes", "node"],
        order: FormatOrder::Published(LOCK_NODE_FIELDS),
    },
    FormatContainerRule {
        path: &["@facts", "fact"],
        order: FormatOrder::Published(LOCK_FACT_FIELDS),
    },
    FormatContainerRule {
        path: &["@occurrences", "occurrence"],
        order: FormatOrder::Published(LOCK_OCCURRENCE_FIELDS),
    },
    FormatContainerRule {
        path: &["@paths", "assertion"],
        order: FormatOrder::Published(LOCK_ASSERTION_FIELDS),
    },
    FormatContainerRule {
        path: &["@mappings", "mapping"],
        order: FormatOrder::Published(LOCK_MAPPING_FIELDS),
    },
    FormatContainerRule {
        path: &["@effective", "resolution"],
        order: FormatOrder::Published(LOCK_RESOLUTION_FIELDS),
    },
    FormatContainerRule {
        path: &["@deployments", "candidate"],
        order: FormatOrder::Published(LOCK_CANDIDATE_FIELDS),
    },
    FormatContainerRule {
        path: &["@themes"],
        order: FormatOrder::Published(&["theme", "contribution", "theme_resolution"]),
    },
    FormatContainerRule {
        path: &["@themes", "theme"],
        order: FormatOrder::Published(LOCK_THEME_FIELDS),
    },
    FormatContainerRule {
        path: &["@themes", "contribution"],
        order: FormatOrder::Published(LOCK_THEME_CONTRIBUTION_FIELDS),
    },
    FormatContainerRule {
        path: &["@themes", "theme_resolution"],
        order: FormatOrder::Published(LOCK_THEME_RESOLUTION_FIELDS),
    },
    FormatContainerRule {
        path: &["@hosts"],
        order: FormatOrder::Published(&["host", "fact"]),
    },
    FormatContainerRule {
        path: &["@hosts", "host"],
        order: FormatOrder::Published(LOCK_HOST_FIELDS),
    },
    FormatContainerRule {
        path: &["@hosts", "fact"],
        order: FormatOrder::Published(LOCK_HOST_FACT_FIELDS),
    },
    FormatContainerRule {
        path: &["@defaults", "default"],
        order: FormatOrder::Published(LOCK_DEFAULT_FIELDS),
    },
];

const PROFILES_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::Profiles,
    root_order: FormatOrder::Published(PROFILES_ROOT),
    container_rules: PROFILES_RULES,
};
const HOSTS_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::Hosts,
    root_order: FormatOrder::NamesByBytes,
    container_rules: HOSTS_RULES,
};
const GROUP_ROOT_REQUIREMENTS_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::GroupRootRequirements,
    root_order: FormatOrder::Requirement(FormatContext::GroupRoot),
    container_rules: GROUP_ROOT_REQUIREMENTS_RULES,
};
const FACET_REQUIREMENTS_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::FacetRequirements,
    root_order: FormatOrder::Requirement(FormatContext::Deployment),
    container_rules: FACET_REQUIREMENTS_RULES,
};
const OVERRIDE_VARIANT_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::OverrideVariant,
    root_order: FormatOrder::Requirement(FormatContext::Deployment),
    container_rules: OVERRIDE_VARIANT_RULES,
};
const RECIPIENT_KEYS_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::RecipientKeys,
    root_order: FormatOrder::Published(&["recipients"]),
    container_rules: RECIPIENT_KEYS_RULES,
};
const SECRET_SCAN_RULES_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::SecretScanRules,
    root_order: FormatOrder::Published(&["allow"]),
    container_rules: SECRET_SCAN_RULES_RULES,
};
const BENCHMARK_BASELINES_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::BenchmarkBaselines,
    root_order: FormatOrder::NamesByBytes,
    container_rules: BENCHMARK_RULES,
};
const THEME_ROLES_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeRoles,
    root_order: FormatOrder::Published(THEME_ROLES_ROOT),
    container_rules: THEME_ROLES_RULES,
};
const THEME_FONTS_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeFonts,
    root_order: FormatOrder::Published(THEME_FONTS_ROOT),
    container_rules: THEME_FONTS_RULES,
};
const THEME_MAP_CATPPUCCIN_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeMapCatppuccin,
    root_order: FormatOrder::Published(&["colors"]),
    container_rules: THEME_MAP_CATPPUCCIN_RULES,
};
const THEME_MAP_EZA_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeMapEza,
    root_order: FormatOrder::Published(&["categories"]),
    container_rules: THEME_MAP_EZA_RULES,
};
const THEME_MAP_GTK_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeMapGtk,
    root_order: FormatOrder::Published(&["colors"]),
    container_rules: THEME_MAP_GTK_RULES,
};
const THEME_MAP_KDE_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeMapKde,
    root_order: FormatOrder::Published(THEME_MAP_KDE_ROOT),
    container_rules: THEME_MAP_KDE_RULES,
};
const THEME_MAP_OBSIDIAN_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeMapObsidian,
    root_order: FormatOrder::Published(&["derived", "variables"]),
    container_rules: THEME_MAP_OBSIDIAN_RULES,
};
const THEME_PROFILE_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::ThemeProfiles,
    root_order: FormatOrder::Published(THEME_PROFILE_ROOT),
    container_rules: THEME_PROFILE_RULES,
};
const TEMPLATE_VARIABLES_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::TemplateVariables,
    root_order: FormatOrder::Published(&["sops_document"]),
    container_rules: &[],
};
const GENERATED_LOCK_SCHEMA: FormatSchema = FormatSchema {
    domain: Domain::GeneratedLock,
    root_order: FormatOrder::Published(GENERATED_LOCK_ROOT),
    container_rules: GENERATED_LOCK_RULES,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::HashSet;

    fn published(order: FormatOrder) -> &'static [&'static str] {
        match order {
            FormatOrder::Published(names) => names,
            other => panic!("expected published order, got {other:?}"),
        }
    }

    #[test]
    fn every_domain_has_one_identity_preserving_schema() {
        assert_eq!(Domain::ALL.len(), 18);
        for domain in Domain::ALL {
            assert_eq!(format_schema(domain).domain, domain);
            assert!(std::ptr::eq(format_schema(domain), format_schema(domain)));
        }
    }

    #[test]
    fn published_root_tables_match_the_frozen_contract() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../../../contracts/dotfile/v1/schemas.json"
        ))
        .expect("schemas.json is valid JSON");

        let published_domains = [
            (Domain::Profiles, "profiles"),
            (Domain::RecipientKeys, "recipient_keys"),
            (Domain::SecretScanRules, "secret_scan_rules"),
            (Domain::ThemeRoles, "theme_roles"),
            (Domain::ThemeFonts, "theme_fonts"),
            (Domain::ThemeMapCatppuccin, "theme_map_catppuccin"),
            (Domain::ThemeMapEza, "theme_map_eza"),
            (Domain::ThemeMapGtk, "theme_map_gtk"),
            (Domain::ThemeMapKde, "theme_map_kde"),
            (Domain::ThemeMapObsidian, "theme_map_obsidian"),
            (Domain::ThemeProfiles, "theme_profile"),
            (Domain::TemplateVariables, "template_variables"),
            (Domain::GeneratedLock, "generated_lock"),
        ];

        for (domain, contract_name) in published_domains {
            let expected: Vec<_> = contract["domain_shapes"][contract_name]["canonical_root_order"]
                .as_array()
                .expect("root order is an array")
                .iter()
                .map(|name| name.as_str().expect("root name is a string"))
                .collect();
            assert_eq!(published(format_schema(domain).root_order), expected);
        }

        assert_eq!(
            format_schema(Domain::Hosts).root_order,
            FormatOrder::NamesByBytes
        );
        assert_eq!(
            format_schema(Domain::BenchmarkBaselines).root_order,
            FormatOrder::NamesByBytes
        );
    }

    #[test]
    fn requirement_attribute_orders_are_frozen_and_contextual() {
        let cases: &[(FormatContext, &[(&str, u16)])] = &[
            (
                FormatContext::Fact,
                &[
                    ("@pkg", 100),
                    ("@installer", 110),
                    ("@bin", 120),
                    ("@check", 130),
                    ("@version", 140),
                    ("@family", 150),
                    ("@service", 160),
                    ("@scope", 170),
                    ("@path", 180),
                    ("@description", 190),
                ],
            ),
            (
                FormatContext::Deployment,
                &[
                    ("@destination", 100),
                    ("@deploy", 110),
                    ("@privilege", 120),
                    ("@sensitivity", 130),
                    ("@mode", 140),
                    ("@owner", 150),
                    ("@group", 160),
                    ("@expect", 170),
                    ("@description", 180),
                    ("@theme", 190),
                ],
            ),
            (
                FormatContext::Group,
                &[
                    ("@directory", 100),
                    ("@os", 110),
                    ("@arch", 120),
                    ("@description", 130),
                ],
            ),
            (
                FormatContext::Profile,
                &[
                    ("@groups", 100),
                    ("@manager", 110),
                    ("@os", 120),
                    ("@arch", 130),
                    ("@theme", 140),
                    ("@description", 150),
                ],
            ),
            (
                FormatContext::Host,
                &[
                    ("hostnames", 80),
                    ("role", 90),
                    ("@profile", 100),
                    ("@theme", 110),
                ],
            ),
            (FormatContext::GroupRoot, &[("@theme", 100)]),
        ];

        for (context, names) in cases {
            let positions: Vec<_> = names
                .iter()
                .map(|(name, _)| attribute_order(*context, name).unwrap())
                .collect();
            assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
            for (name, position) in *names {
                assert_eq!(attribute_order(*context, name), Some(*position));
                assert_eq!(
                    FormatSchema::attribute_order(*context, name),
                    Some(*position)
                );
            }
            assert_eq!(attribute_order(*context, "unknown"), None);
        }

        assert_eq!(attribute_order(FormatContext::Fact, "@theme"), None);
        assert_eq!(attribute_order(FormatContext::Deployment, "@pkg"), None);
    }

    #[test]
    fn wildcard_rules_are_one_component_and_literals_win() {
        static RULES: &[FormatContainerRule] = &[
            FormatContainerRule {
                path: &["*"],
                order: FormatOrder::NamesByBytes,
            },
            FormatContainerRule {
                path: &["known"],
                order: FormatOrder::Published(&["a", "b"]),
            },
        ];
        let schema = FormatSchema {
            domain: Domain::Hosts,
            root_order: FormatOrder::Preserve,
            container_rules: RULES,
        };

        assert_eq!(schema.order_for(&[]), FormatOrder::Preserve);
        assert_eq!(schema.order_for(&["dynamic"]), FormatOrder::NamesByBytes);
        assert_eq!(
            schema.order_for(&["known"]),
            FormatOrder::Published(&["a", "b"])
        );
        assert_eq!(schema.order_for(&["one", "two"]), FormatOrder::Preserve);
    }

    #[test]
    fn profiles_hosts_and_requirements_select_semantic_contexts() {
        assert_eq!(
            order_for(Domain::Profiles, &["@groups", "desktop"]),
            FormatOrder::Requirement(FormatContext::Group)
        );
        assert_eq!(
            order_for(Domain::Profiles, &["@profiles", "laptop"]),
            FormatOrder::Requirement(FormatContext::Profile)
        );
        assert_eq!(
            order_for(Domain::Hosts, &["archie"]),
            FormatOrder::Requirement(FormatContext::Host)
        );
        assert_eq!(
            order_for(Domain::FacetRequirements, &[]),
            FormatOrder::Requirement(FormatContext::Deployment)
        );
        assert_eq!(
            order_for(Domain::FacetRequirements, &["resource_demand"]),
            FormatOrder::Requirement(FormatContext::Fact)
        );
        assert_eq!(
            order_for(Domain::FacetRequirements, &["path"]),
            FormatOrder::Requirement(FormatContext::Deployment)
        );
    }

    #[test]
    fn requirement_category_legality_is_path_and_domain_specific() {
        use FormatEntryKind::{Demand, Extension, Path, Resource, ResourceIdentity};

        let root_cases = [
            (
                Domain::GroupRootRequirements,
                [
                    (Path, false),
                    (Resource, true),
                    (Extension, true),
                    (Demand, true),
                ],
            ),
            (
                Domain::FacetRequirements,
                [
                    (Path, true),
                    (Resource, true),
                    (Extension, true),
                    (Demand, true),
                ],
            ),
            (
                Domain::OverrideVariant,
                [
                    (Path, true),
                    (Resource, false),
                    (Extension, false),
                    (Demand, false),
                ],
            ),
        ];
        for (domain, cases) in root_cases {
            for (kind, allowed) in cases {
                assert_eq!(entry_allowed(domain, &[], kind), allowed);
                assert!(!entry_allowed(domain, &[], ResourceIdentity));
            }
        }

        for domain in [Domain::GroupRootRequirements, Domain::FacetRequirements] {
            assert!(entry_allowed(domain, &["entity_demand"], Demand));
            assert!(!entry_allowed(domain, &["entity_demand"], Path));
            assert!(entry_allowed(domain, &["entity_demand"], Resource));
            assert!(!entry_allowed(domain, &["entity_demand"], Extension));
            assert!(!entry_allowed(domain, &["entity_demand"], ResourceIdentity));

            assert!(entry_allowed(
                domain,
                &["resource_demand"],
                ResourceIdentity
            ));
            for container in ["resource_demand", "@extend", "path"] {
                for kind in [Path, Resource, Extension, Demand] {
                    assert!(!entry_allowed(domain, &[container], kind));
                }
            }

            assert!(entry_allowed(
                domain,
                &["entity_demand", "entity_demand"],
                Demand
            ));
            assert!(entry_allowed(
                domain,
                &["entity_demand", "entity_demand"],
                Resource
            ));
        }

        assert!(!entry_allowed(
            Domain::OverrideVariant,
            &["entity_demand"],
            Demand
        ));
        assert!(!entry_allowed(Domain::ThemeRoles, &[], Demand));
    }

    #[test]
    fn semantic_source_order_is_never_accidentally_sorted() {
        let preserved = [
            (Domain::Profiles, &["@groups"][..]),
            (Domain::Profiles, &["@profiles"][..]),
            (Domain::SecretScanRules, &["allow"][..]),
            (Domain::ThemeRoles, &["roles"][..]),
            (Domain::ThemeRoles, &["eza", "categories"][..]),
            (Domain::ThemeFonts, &["fonts"][..]),
            (Domain::ThemeFonts, &["applications"][..]),
            (Domain::ThemeMapCatppuccin, &["colors"][..]),
            (Domain::ThemeMapEza, &["categories"][..]),
            (Domain::ThemeMapGtk, &["colors"][..]),
            (Domain::ThemeMapKde, &["groups"][..]),
            (Domain::ThemeMapObsidian, &["variables"][..]),
            (Domain::ThemeProfiles, &["palette"][..]),
            (Domain::ThemeProfiles, &["applications"][..]),
        ];
        for (domain, path) in preserved {
            assert_eq!(order_for(domain, path), FormatOrder::Preserve);
        }
    }

    #[test]
    fn open_role_regions_precede_their_published_structural_tail() {
        for domain in [Domain::ThemeRoles, Domain::ThemeProfiles] {
            assert_eq!(
                order_for(domain, &["terminal"]),
                FormatOrder::OpenThenPublished(&["ansi", "tabs"])
            );
            assert_eq!(
                order_for(domain, &["eza"]),
                FormatOrder::OpenThenPublished(&["categories", "pattern"])
            );
        }
    }

    #[test]
    fn nested_profile_groups_keep_group_attribute_legality() {
        for path in [
            &["@groups", "linux"][..],
            &["@groups", "linux", "desktop"][..],
            &["@groups", "linux", "desktop", "graphical"][..],
        ] {
            assert!(attribute_allowed(Domain::Profiles, path, "@directory"));
            assert!(attribute_allowed(Domain::Profiles, path, "@os"));
            assert!(!attribute_allowed(Domain::Profiles, path, "@manager"));
        }
    }

    #[test]
    fn generated_lock_exposes_every_record_field_layout() {
        let expected = [
            ("@lock", None, LOCK_HEADER_FIELDS),
            ("@sources", Some("source"), LOCK_SOURCE_FIELDS),
            ("@groups", Some("group"), LOCK_GROUP_FIELDS),
            ("@profiles", Some("profile"), LOCK_PROFILE_FIELDS),
            ("@facets", Some("facet"), LOCK_FACET_FIELDS),
            ("@nodes", Some("node"), LOCK_NODE_FIELDS),
            ("@facts", Some("fact"), LOCK_FACT_FIELDS),
            ("@occurrences", Some("occurrence"), LOCK_OCCURRENCE_FIELDS),
            ("@paths", Some("assertion"), LOCK_ASSERTION_FIELDS),
            ("@mappings", Some("mapping"), LOCK_MAPPING_FIELDS),
            ("@effective", Some("resolution"), LOCK_RESOLUTION_FIELDS),
            ("@deployments", Some("candidate"), LOCK_CANDIDATE_FIELDS),
            ("@themes", Some("theme"), LOCK_THEME_FIELDS),
            (
                "@themes",
                Some("contribution"),
                LOCK_THEME_CONTRIBUTION_FIELDS,
            ),
            (
                "@themes",
                Some("theme_resolution"),
                LOCK_THEME_RESOLUTION_FIELDS,
            ),
            ("@hosts", Some("host"), LOCK_HOST_FIELDS),
            ("@hosts", Some("fact"), LOCK_HOST_FACT_FIELDS),
            ("@defaults", Some("default"), LOCK_DEFAULT_FIELDS),
        ];

        for (section, record, fields) in expected {
            let path = record.map_or_else(|| vec![section], |record| vec![section, record]);
            assert_eq!(published(order_for(Domain::GeneratedLock, &path)), fields);
        }
    }

    #[test]
    fn registry_paths_are_unique_within_each_domain() {
        for domain in Domain::ALL {
            let mut paths = HashSet::new();
            for rule in format_schema(domain).container_rules {
                assert!(
                    paths.insert(rule.path),
                    "duplicate formatter path in {domain}: {:?}",
                    rule.path
                );
            }
        }
    }
}
