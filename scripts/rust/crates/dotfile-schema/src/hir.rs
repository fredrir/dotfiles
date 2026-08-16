use std::collections::{BTreeMap, HashMap};

use dotfile_source::{ByteRange, Diagnostic, RepoPath, SourceText};
use dotfile_syntax::{NodeId, NodeKind, TokenKind};
use serde_json::{Value as JsonValue, json};

use crate::{ClassifiedPath, Domain};

/// Dense identity of one owned HIR object within a [`HirFile`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HirId(pub u32);

impl HirId {
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScopeId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BindingId(pub u32);

/// Why a tolerant HIR object cannot participate in validated semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoisonKind {
    Syntax,
    Missing,
    Context,
    Duplicate,
    Value,
    Binding,
}

impl PoisonKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::Missing => "missing",
            Self::Context => "context",
            Self::Duplicate => "duplicate",
            Self::Value => "value",
            Self::Binding => "binding",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Poison {
    pub kind: PoisonKind,
    pub range: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoisonNode {
    pub id: HirId,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

/// Typed recovery objects retained when an entry cannot occupy its normal
/// singleton/domain slot. Keeping these objects owned by the file prevents
/// source-map identities from outliving the HIR nodes they identify.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryNode {
    Attribute(Attribute),
    NamedField(NamedField),
    Poison(PoisonNode),
}

impl RecoveryNode {
    pub fn hir_id(&self) -> HirId {
        match self {
            Self::Attribute(value) => value.id,
            Self::NamedField(value) => value.id,
            Self::Poison(value) => value.id,
        }
    }
}

/// Source identity for one HIR object.  String-token segments have no CST
/// node identity and therefore use `syntax = None` with their raw range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceOrigin {
    pub syntax: Option<NodeId>,
    pub range: ByteRange,
}

/// Bidirectional mapping between syntax identity/raw source ranges and HIR.
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    next_hir: u32,
    hir_to_source: BTreeMap<HirId, SourceOrigin>,
    syntax_to_hir: HashMap<NodeId, Vec<HirId>>,
    range_to_hir: BTreeMap<ByteRange, Vec<HirId>>,
}

impl SourceMap {
    pub(crate) fn allocate(&mut self, syntax: Option<NodeId>, range: ByteRange) -> HirId {
        let id = HirId(self.next_hir);
        self.next_hir = self.next_hir.checked_add(1).expect("HIR id overflow");
        self.insert(id, SourceOrigin { syntax, range });
        id
    }

    pub(crate) fn insert(&mut self, hir: HirId, origin: SourceOrigin) {
        self.next_hir = self
            .next_hir
            .max(hir.0.checked_add(1).expect("HIR id overflow"));
        self.hir_to_source.insert(hir, origin);
        if let Some(syntax) = origin.syntax {
            let ids = self.syntax_to_hir.entry(syntax).or_default();
            if !ids.contains(&hir) {
                ids.push(hir);
                ids.sort_unstable();
            }
        }
        let ids = self.range_to_hir.entry(origin.range).or_default();
        if !ids.contains(&hir) {
            ids.push(hir);
            ids.sort_unstable();
        }
    }

    pub(crate) fn insert_range(&mut self, hir: HirId, range: ByteRange) {
        let ids = self.range_to_hir.entry(range).or_default();
        if !ids.contains(&hir) {
            ids.push(hir);
            ids.sort_unstable();
        }
    }

    pub fn source_for_hir(&self, hir: HirId) -> Option<SourceOrigin> {
        self.hir_to_source.get(&hir).copied()
    }

    pub fn hir_for_syntax(&self, syntax: NodeId) -> &[HirId] {
        self.syntax_to_hir
            .get(&syntax)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn hir_for_range(&self, range: ByteRange) -> &[HirId] {
        self.range_to_hir
            .get(&range)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn hir_origins(&self) -> impl Iterator<Item = (HirId, SourceOrigin)> + '_ {
        self.hir_to_source
            .iter()
            .map(|(hir, origin)| (*hir, *origin))
    }

    pub fn syntax_mappings(&self) -> impl Iterator<Item = (NodeId, &[HirId])> + '_ {
        let mut entries: Vec<_> = self
            .syntax_to_hir
            .iter()
            .map(|(syntax, ids)| (*syntax, ids.as_slice()))
            .collect();
        entries.sort_by_key(|(syntax, _)| *syntax);
        entries.into_iter()
    }

    pub fn range_mappings(&self) -> impl Iterator<Item = (ByteRange, &[HirId])> + '_ {
        self.range_to_hir
            .iter()
            .map(|(range, ids)| (*range, ids.as_slice()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceNamespace {
    ResourceKey,
    Group,
    Theme,
    Profile,
}

impl ReferenceNamespace {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResourceKey => "resource_key",
            Self::Group => "group",
            Self::Theme => "theme",
            Self::Profile => "profile",
        }
    }
}

/// Closed v1 schema value kinds.  This is type information, not resolved
/// cross-file semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueType {
    DotfileVersion,
    String,
    OneLineString,
    StringExpression,
    CommandName,
    Check,
    StringOrStringList,
    ResourceKeyReference,
    MachinePath,
    DestinationPath,
    RepositoryDirectory,
    Deploy,
    Privilege,
    Sensitivity,
    Mode,
    Expect,
    ServiceScope,
    Os,
    ArchitectureList,
    GroupReferenceList,
    GroupReference,
    Manager,
    Installer,
    ThemeReference,
    ProfileReference,
    Hostnames,
    HostRole,
    HostExtension,
    Recipient,
    ScanPattern,
    Inspect,
    BenchmarkRunId,
}

impl ValueType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DotfileVersion => "dotfile_version_literal",
            Self::String => "string",
            Self::OneLineString => "one_line_string",
            Self::StringExpression => "string_expression",
            Self::CommandName => "command_name_string",
            Self::Check => "check_enum",
            Self::StringOrStringList => "string_or_string_list",
            Self::ResourceKeyReference => "bare_resource_key_reference",
            Self::MachinePath => "machine_path_expression",
            Self::DestinationPath => "destination_path_expression",
            Self::RepositoryDirectory => "repository_directory_string",
            Self::Deploy => "deploy_enum",
            Self::Privilege => "privilege_enum",
            Self::Sensitivity => "sensitivity_enum",
            Self::Mode => "mode_string",
            Self::Expect => "expect_enum",
            Self::ServiceScope => "service_scope_enum",
            Self::Os => "os_enum",
            Self::ArchitectureList => "architecture_list",
            Self::GroupReferenceList => "group_reference_list",
            Self::GroupReference => "group_reference",
            Self::Manager => "manager_string",
            Self::Installer => "installer_string",
            Self::ThemeReference => "theme_reference",
            Self::ProfileReference => "profile_reference",
            Self::Hostnames => "hostnames",
            Self::HostRole => "host_role",
            Self::HostExtension => "host_extension",
            Self::Recipient => "age_public_recipient",
            Self::ScanPattern => "scan_pattern",
            Self::Inspect => "scan_inspect",
            Self::BenchmarkRunId => "benchmark_run_id",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceValue {
    pub name: String,
    pub namespace: ReferenceNamespace,
    pub range: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindingResolution {
    Resolved(BindingId),
    SelfReference,
    UseBeforeDeclaration(BindingId),
    Unbound,
    Poison,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionSegment {
    Literal {
        text: String,
        range: ByteRange,
    },
    Binding {
        name: String,
        range: ByteRange,
        resolution: BindingResolution,
    },
    Poison {
        range: ByteRange,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingEdge {
    pub binding: BindingId,
    pub declaration_range: ByteRange,
    pub reference_range: ByteRange,
}

/// One contiguous output segment with its definition/reference provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluatedSegment {
    pub text: String,
    pub source_range: ByteRange,
    pub binding_edges: Vec<BindingEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluatedString {
    pub value: String,
    pub segments: Vec<EvaluatedSegment>,
    pub poisoned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringExpression {
    pub range: ByteRange,
    pub segments: Vec<ExpressionSegment>,
    pub evaluated: EvaluatedString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HirValueKind {
    String(StringExpression),
    Reference(ReferenceValue),
    List(Vec<HirValue>),
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirValue {
    pub id: HirId,
    pub value_type: ValueType,
    pub kind: HirValueKind,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttributeKind {
    DotfileVersion,
    Pkg,
    Installer,
    Bin,
    Check,
    Version,
    Family,
    Service,
    Scope,
    Path,
    Key,
    Destination,
    Deploy,
    Privilege,
    Sensitivity,
    Mode,
    Owner,
    Group,
    Expect,
    Directory,
    Os,
    Arch,
    Groups,
    Manager,
    Profile,
    Theme,
    Description,
}

impl AttributeKind {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "dotfile-version" => Self::DotfileVersion,
            "pkg" => Self::Pkg,
            "installer" => Self::Installer,
            "bin" => Self::Bin,
            "check" => Self::Check,
            "version" => Self::Version,
            "family" => Self::Family,
            "service" => Self::Service,
            "scope" => Self::Scope,
            "path" => Self::Path,
            "key" => Self::Key,
            "destination" => Self::Destination,
            "deploy" => Self::Deploy,
            "privilege" => Self::Privilege,
            "sensitivity" => Self::Sensitivity,
            "mode" => Self::Mode,
            "owner" => Self::Owner,
            "group" => Self::Group,
            "expect" => Self::Expect,
            "directory" => Self::Directory,
            "os" => Self::Os,
            "arch" => Self::Arch,
            "groups" => Self::Groups,
            "manager" => Self::Manager,
            "profile" => Self::Profile,
            "theme" => Self::Theme,
            "description" => Self::Description,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DotfileVersion => "@dotfile-version",
            Self::Pkg => "@pkg",
            Self::Installer => "@installer",
            Self::Bin => "@bin",
            Self::Check => "@check",
            Self::Version => "@version",
            Self::Family => "@family",
            Self::Service => "@service",
            Self::Scope => "@scope",
            Self::Path => "@path",
            Self::Key => "@key",
            Self::Destination => "@destination",
            Self::Deploy => "@deploy",
            Self::Privilege => "@privilege",
            Self::Sensitivity => "@sensitivity",
            Self::Mode => "@mode",
            Self::Owner => "@owner",
            Self::Group => "@group",
            Self::Expect => "@expect",
            Self::Directory => "@directory",
            Self::Os => "@os",
            Self::Arch => "@arch",
            Self::Groups => "@groups",
            Self::Manager => "@manager",
            Self::Profile => "@profile",
            Self::Theme => "@theme",
            Self::Description => "@description",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub id: HirId,
    pub kind: AttributeKind,
    pub value: HirValue,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingDeclaration {
    pub id: BindingId,
    pub hir_id: HirId,
    pub scope: ScopeId,
    pub name: String,
    pub name_range: ByteRange,
    pub range: ByteRange,
    pub initializer: StringExpression,
    pub evaluated: EvaluatedString,
    pub used: bool,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingScope {
    pub id: ScopeId,
    pub hir_id: HirId,
    pub parent: Option<ScopeId>,
    pub range: ByteRange,
    pub prologue_end: u64,
    pub bindings: Vec<BindingId>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profiles {
    pub id: HirId,
    pub version: Option<Attribute>,
    pub groups: Vec<GroupDeclaration>,
    pub profiles: Vec<ProfileDeclaration>,
    pub theme: Option<Attribute>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupDeclaration {
    pub id: HirId,
    pub name: String,
    pub name_range: ByteRange,
    pub parent: Option<HirId>,
    pub attributes: Vec<Attribute>,
    pub children: Vec<GroupDeclaration>,
    pub poison: Vec<Poison>,
}

impl GroupDeclaration {
    pub fn attribute(&self, kind: AttributeKind) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|attribute| attribute.kind == kind)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileDeclaration {
    pub id: HirId,
    pub name: String,
    pub name_range: ByteRange,
    pub attributes: Vec<Attribute>,
    pub poison: Vec<Poison>,
}

impl ProfileDeclaration {
    pub fn attribute(&self, kind: AttributeKind) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|attribute| attribute.kind == kind)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hosts {
    pub id: HirId,
    pub hosts: Vec<HostDeclaration>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDeclaration {
    pub id: HirId,
    pub name: String,
    pub name_range: ByteRange,
    pub hostnames: Option<NamedField>,
    pub role: Option<NamedField>,
    pub profile: Option<Attribute>,
    pub theme: Option<Attribute>,
    pub extensions: Vec<NamedField>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedField {
    pub id: HirId,
    pub name: String,
    pub name_range: ByteRange,
    pub value: HirValue,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequirementContext {
    GroupRoot,
    Facet,
    Variant,
    EntityFact,
    ResourceFact,
    Extension,
    Path,
}

impl RequirementContext {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GroupRoot => "group_root",
            Self::Facet => "facet",
            Self::Variant => "variant",
            Self::EntityFact => "entity_fact",
            Self::ResourceFact => "resource_fact",
            Self::Extension => "extension",
            Self::Path => "path",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Requirements {
    pub id: HirId,
    pub context: RequirementContext,
    pub scope: ScopeId,
    pub entries: Vec<RequirementEntry>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequirementEntry {
    Binding(BindingId),
    Attribute(Attribute),
    Entity(EntityDemand),
    Resource(ResourceDemand),
    Extension(Extension),
    Path(PathNode),
    Poison(PoisonNode),
}

impl RequirementEntry {
    pub fn hir_id(&self, bindings: &[BindingDeclaration]) -> HirId {
        match self {
            Self::Binding(id) => bindings[id.0 as usize].hir_id,
            Self::Attribute(value) => value.id,
            Self::Entity(value) => value.id,
            Self::Resource(value) => value.id,
            Self::Extension(value) => value.id,
            Self::Path(value) => value.id,
            Self::Poison(value) => value.id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityDemand {
    pub id: HirId,
    pub name: String,
    pub name_range: ByteRange,
    pub optional: bool,
    pub assignment_sugar: bool,
    pub scope: Option<ScopeId>,
    pub entries: Vec<RequirementEntry>,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceDemand {
    pub id: HirId,
    pub kind: String,
    pub optional: bool,
    pub key: Option<ReferenceValue>,
    pub scope: ScopeId,
    pub entries: Vec<RequirementEntry>,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Extension {
    pub id: HirId,
    pub namespace: String,
    pub name: String,
    pub scope: ScopeId,
    pub entries: Vec<RequirementEntry>,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathNode {
    pub id: HirId,
    pub path: String,
    pub optional: bool,
    pub scope: ScopeId,
    pub entries: Vec<RequirementEntry>,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecipientKeys {
    pub id: HirId,
    pub entries: Vec<NamedField>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretScanRules {
    pub id: HirId,
    pub rules: Vec<ScanRule>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanRule {
    pub id: HirId,
    pub pattern: Option<NamedField>,
    pub inspect: Option<NamedField>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkBaselines {
    pub id: HirId,
    pub hosts: Vec<BenchmarkHost>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkHost {
    pub id: HirId,
    pub name: String,
    pub name_range: ByteRange,
    pub epochs: Vec<NamedField>,
    pub poison: Vec<Poison>,
}

/// Domains whose specialized validation is owned downstream still receive
/// an owned, source-mapped root and explicit identity metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredDomain {
    pub id: HirId,
    pub domain: Domain,
    pub identity: Option<String>,
    /// Owned syntax skeleton for domains whose semantic schema is lowered by
    /// a downstream crate. Every descendant CST node has an authoritative
    /// HIR identity and source-map entry that the specialized lowerer can
    /// reuse without retaining CST references.
    pub syntax: Vec<DeferredSyntaxNode>,
    /// Recovered zero-width terminals directly owned by the file node.
    pub missing: Vec<DeferredMissing>,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredSyntaxNode {
    pub id: HirId,
    pub kind: NodeKind,
    pub range: ByteRange,
    pub children: Vec<DeferredSyntaxNode>,
    /// Recovered zero-width terminals directly owned by this syntax node.
    pub missing: Vec<DeferredMissing>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredMissing {
    pub id: HirId,
    pub expected: TokenKind,
    pub range: ByteRange,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HirRoot {
    Profiles(Box<Profiles>),
    Hosts(Box<Hosts>),
    Requirements(Box<Requirements>),
    RecipientKeys(Box<RecipientKeys>),
    SecretScanRules(Box<SecretScanRules>),
    BenchmarkBaselines(Box<BenchmarkBaselines>),
    Deferred(DeferredDomain),
    Unknown(PoisonNode),
}

impl HirRoot {
    pub fn hir_id(&self) -> HirId {
        match self {
            Self::Profiles(value) => value.id,
            Self::Hosts(value) => value.id,
            Self::Requirements(value) => value.id,
            Self::RecipientKeys(value) => value.id,
            Self::SecretScanRules(value) => value.id,
            Self::BenchmarkBaselines(value) => value.id,
            Self::Deferred(value) => value.id,
            Self::Unknown(value) => value.id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirFile {
    pub root: HirRoot,
    pub scopes: Vec<BindingScope>,
    pub bindings: Vec<BindingDeclaration>,
    pub recovery: Vec<RecoveryNode>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug)]
pub struct LoweredFile {
    pub(crate) path: RepoPath,
    pub(crate) parsed_source: SourceText,
    pub(crate) classification: Option<ClassifiedPath>,
    pub(crate) hir: HirFile,
    pub(crate) source_map: SourceMap,
    /// Schema-stage diagnostics only.  Lex/parse diagnostics remain on the
    /// parser result and are combined by the caller's pipeline.
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// A typed file proven free of lexer, parser, and schema errors.
/// Construction is intentionally restricted to [`LoweredFile::into_validated`].
#[derive(Clone, Debug)]
pub struct ValidatedFile {
    lowered: LoweredFile,
}

#[derive(Clone, Debug)]
pub struct ValidationFailure {
    lowered: Box<LoweredFile>,
}

impl ValidationFailure {
    pub fn lowered(&self) -> &LoweredFile {
        &self.lowered
    }

    pub fn into_lowered(self) -> LoweredFile {
        *self.lowered
    }
}

impl ValidatedFile {
    pub fn lowered(&self) -> &LoweredFile {
        &self.lowered
    }

    pub fn hir(&self) -> &HirFile {
        &self.lowered.hir
    }

    pub fn into_lowered(self) -> LoweredFile {
        self.lowered
    }
}

impl LoweredFile {
    pub fn path(&self) -> &RepoPath {
        &self.path
    }

    pub fn classification(&self) -> Option<&ClassifiedPath> {
        self.classification.as_ref()
    }

    pub fn hir(&self) -> &HirFile {
        &self.hir
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn domain(&self) -> Option<Domain> {
        self.classification.as_ref().map(|value| value.domain)
    }

    /// Whether this HIR was lowered from exactly these immutable source
    /// bytes. Downstream specialized lowerers use this before enriching a
    /// deferred root.
    pub fn was_lowered_from(&self, source: &SourceText) -> bool {
        self.parsed_source == *source
    }

    /// Read-only provenance for this owned HIR. Specialized downstream
    /// lowerers enrich it only through [`Self::allocate_synthetic_hir`].
    pub fn source_map(&self) -> &SourceMap {
        &self.source_map
    }

    pub fn source_len(&self) -> u64 {
        self.parsed_source.len()
    }

    /// Verifies the sealed invariants of a deferred schema snapshot before a
    /// downstream domain lowerer enriches or validates it.
    pub fn deferred_snapshot_is_authoritative(&self) -> bool {
        let (Some(classification), HirRoot::Deferred(deferred)) =
            (&self.classification, &self.hir.root)
        else {
            return false;
        };
        if deferred.domain != classification.domain
            || deferred.range.end() > self.parsed_source.len()
            || !origin_matches(&self.source_map, deferred.id, deferred.range, true)
            || self
                .source_map
                .hir_origins()
                .any(|(_, origin)| origin.range.end() > self.parsed_source.len())
        {
            return false;
        }

        let mut ids = std::collections::HashSet::new();
        if !ids.insert(deferred.id) {
            return false;
        }
        deferred_syntax_is_authoritative(
            &deferred.syntax,
            &deferred.missing,
            &self.source_map,
            self.parsed_source.len(),
            &mut ids,
        ) && ids.len() == self.source_map.hir_origins().count()
    }

    /// Allocates a source-mapped identity for a semantic node that has no
    /// authored CST node, such as a required-but-absent field. Present nodes
    /// must reuse the identity already mapped from their syntax node.
    pub fn allocate_synthetic_hir(&mut self, range: ByteRange) -> Option<HirId> {
        (matches!(self.hir.root, HirRoot::Deferred(_)) && range.end() <= self.parsed_source.len())
            .then(|| self.source_map.allocate(None, range))
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == dotfile_source::Severity::Error)
    }

    /// Seals this HIR only when both upstream syntax and schema validation
    /// are error-free.  On failure the tolerant file is returned unchanged.
    pub fn into_validated(
        self,
        parse: &dotfile_syntax::Parse,
    ) -> Result<ValidatedFile, ValidationFailure> {
        if self.classification.is_none()
            || !parse.was_parsed_from(&self.path, &self.parsed_source)
            || parse.has_errors()
            || self.has_errors()
            || !self.hir.recovery.is_empty()
            || !self.hir.poison.is_empty()
            || matches!(&self.hir.root, HirRoot::Deferred(_))
        {
            Err(ValidationFailure {
                lowered: Box::new(self),
            })
        } else {
            Ok(ValidatedFile { lowered: self })
        }
    }

    /// Deterministic fixture/debug projection.  This is deliberately not a
    /// production serialization contract.
    pub fn dump_json(&self) -> JsonValue {
        let mut source_map: Vec<_> = self
            .source_map
            .hir_origins()
            .map(|(hir, origin)| {
                json!({
                    "hir": hir.get(),
                    "syntax": origin.syntax.map(|id| format!("{id:?}")),
                    "range": range_json(origin.range),
                    "hir_ids_for_range": self.source_map.hir_for_range(origin.range)
                        .iter().map(|id| id.get()).collect::<Vec<_>>(),
                    "hir_ids_for_syntax": origin.syntax
                        .map(|syntax| self.source_map.hir_for_syntax(syntax)
                            .iter().map(|id| id.get()).collect::<Vec<_>>())
                        .unwrap_or_default(),
                })
            })
            .collect();
        source_map.sort_by_key(|value| value["hir"].as_u64().unwrap_or(0));

        let scopes: Vec<_> = self
            .hir
            .scopes
            .iter()
            .map(|scope| {
                json!({
                    "id": scope.id.0,
                    "hir": scope.hir_id.get(),
                    "parent": scope.parent.map(|id| id.0),
                    "range": range_json(scope.range),
                    "prologue_end": scope.prologue_end,
                    "bindings": scope.bindings.iter().map(|id| id.0).collect::<Vec<_>>(),
                    "poison": poison_json(&scope.poison),
                })
            })
            .collect();
        let bindings: Vec<_> = self.hir.bindings.iter().map(binding_json).collect();

        json!({
            "path": self.path.as_str(),
            "domain": self.domain().map(Domain::as_str),
            "location": self.classification.as_ref().map(|value| format!("{:?}", value.location)),
            "root": root_json(&self.hir.root),
            "scopes": scopes,
            "bindings": bindings,
            "recovery": self.hir.recovery.iter().map(recovery_json).collect::<Vec<_>>(),
            "poison": poison_json(&self.hir.poison),
            "source_map": source_map,
            "diagnostics": self.diagnostics.iter().map(|diagnostic| {
                serde_json::to_value(diagnostic).expect("Diagnostic is serializable")
            }).collect::<Vec<_>>(),
        })
    }
}

fn recovery_json(value: &RecoveryNode) -> JsonValue {
    match value {
        RecoveryNode::Attribute(value) => json!({
            "kind": "attribute",
            "value": attribute_json(value),
        }),
        RecoveryNode::NamedField(value) => json!({
            "kind": "named_field",
            "value": field_json(value),
        }),
        RecoveryNode::Poison(value) => json!({
            "kind": "poison",
            "id": value.id.get(),
            "range": range_json(value.range),
            "poison": poison_json(&value.poison),
        }),
    }
}

fn origin_matches(source_map: &SourceMap, id: HirId, range: ByteRange, authored: bool) -> bool {
    let Some(origin) = source_map.source_for_hir(id) else {
        return false;
    };
    origin.range == range
        && origin.syntax.is_some() == authored
        && source_map.hir_for_range(range).contains(&id)
        && origin
            .syntax
            .is_none_or(|syntax| source_map.hir_for_syntax(syntax).contains(&id))
}

fn deferred_syntax_is_authoritative(
    nodes: &[DeferredSyntaxNode],
    missing: &[DeferredMissing],
    source_map: &SourceMap,
    source_len: u64,
    ids: &mut std::collections::HashSet<HirId>,
) -> bool {
    for item in missing {
        if item.range.end() > source_len
            || !ids.insert(item.id)
            || !origin_matches(source_map, item.id, item.range, false)
            || item
                .poison
                .iter()
                .any(|poison| poison.range.end() > source_len)
        {
            return false;
        }
    }
    for node in nodes {
        if node.range.end() > source_len
            || !ids.insert(node.id)
            || !origin_matches(source_map, node.id, node.range, true)
            || node
                .poison
                .iter()
                .any(|poison| poison.range.end() > source_len)
            || !deferred_syntax_is_authoritative(
                &node.children,
                &node.missing,
                source_map,
                source_len,
                ids,
            )
        {
            return false;
        }
    }
    true
}

fn root_json(root: &HirRoot) -> JsonValue {
    match root {
        HirRoot::Profiles(value) => json!({
            "kind": "profiles",
            "id": value.id.get(),
            "version": value.version.as_ref().map(attribute_json),
            "groups": value.groups.iter().map(group_json).collect::<Vec<_>>(),
            "profiles": value.profiles.iter().map(profile_json).collect::<Vec<_>>(),
            "theme": value.theme.as_ref().map(attribute_json),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::Hosts(value) => json!({
            "kind": "hosts",
            "id": value.id.get(),
            "hosts": value.hosts.iter().map(host_json).collect::<Vec<_>>(),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::Requirements(value) => json!({
            "kind": "requirements",
            "id": value.id.get(),
            "context": value.context.as_str(),
            "scope": value.scope.0,
            "entries": value.entries.iter().map(requirement_json).collect::<Vec<_>>(),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::RecipientKeys(value) => json!({
            "kind": "recipient_keys", "id": value.id.get(),
            "entries": value.entries.iter().map(field_json).collect::<Vec<_>>(),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::SecretScanRules(value) => json!({
            "kind": "secret_scan_rules", "id": value.id.get(),
            "rules": value.rules.iter().map(|rule| json!({
                "id": rule.id.get(),
                "pattern": rule.pattern.as_ref().map(field_json),
                "inspect": rule.inspect.as_ref().map(field_json),
                "poison": poison_json(&rule.poison),
            })).collect::<Vec<_>>(),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::BenchmarkBaselines(value) => json!({
            "kind": "benchmark_baselines", "id": value.id.get(),
            "hosts": value.hosts.iter().map(|host| json!({
                "id": host.id.get(), "name": host.name,
                "epochs": host.epochs.iter().map(field_json).collect::<Vec<_>>(),
                "poison": poison_json(&host.poison),
            })).collect::<Vec<_>>(),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::Deferred(value) => json!({
            "kind": "deferred", "id": value.id.get(), "domain": value.domain.as_str(),
            "identity": value.identity, "range": range_json(value.range),
            "syntax": value.syntax.iter().map(deferred_syntax_json).collect::<Vec<_>>(),
            "missing": value.missing.iter().map(deferred_missing_json).collect::<Vec<_>>(),
            "poison": poison_json(&value.poison),
        }),
        HirRoot::Unknown(value) => json!({
            "kind": "unknown", "id": value.id.get(), "range": range_json(value.range),
            "poison": poison_json(&value.poison),
        }),
    }
}

fn deferred_syntax_json(value: &DeferredSyntaxNode) -> JsonValue {
    json!({
        "id": value.id.get(),
        "kind": value.kind.name(),
        "range": range_json(value.range),
        "children": value.children.iter().map(deferred_syntax_json).collect::<Vec<_>>(),
        "missing": value.missing.iter().map(deferred_missing_json).collect::<Vec<_>>(),
        "poison": poison_json(&value.poison),
    })
}

fn deferred_missing_json(value: &DeferredMissing) -> JsonValue {
    json!({
        "id": value.id.get(),
        "expected": value.expected.name(),
        "range": range_json(value.range),
        "poison": poison_json(&value.poison),
    })
}

fn group_json(value: &GroupDeclaration) -> JsonValue {
    json!({
        "id": value.id.get(), "name": value.name,
        "parent": value.parent.map(HirId::get),
        "attributes": value.attributes.iter().map(attribute_json).collect::<Vec<_>>(),
        "children": value.children.iter().map(group_json).collect::<Vec<_>>(),
        "poison": poison_json(&value.poison),
    })
}

fn profile_json(value: &ProfileDeclaration) -> JsonValue {
    json!({
        "id": value.id.get(), "name": value.name,
        "attributes": value.attributes.iter().map(attribute_json).collect::<Vec<_>>(),
        "poison": poison_json(&value.poison),
    })
}

fn host_json(value: &HostDeclaration) -> JsonValue {
    json!({
        "id": value.id.get(), "name": value.name,
        "hostnames": value.hostnames.as_ref().map(field_json),
        "role": value.role.as_ref().map(field_json),
        "profile": value.profile.as_ref().map(attribute_json),
        "theme": value.theme.as_ref().map(attribute_json),
        "extensions": value.extensions.iter().map(field_json).collect::<Vec<_>>(),
        "poison": poison_json(&value.poison),
    })
}

fn requirement_json(value: &RequirementEntry) -> JsonValue {
    match value {
        RequirementEntry::Binding(id) => json!({"kind":"binding", "id": id.0}),
        RequirementEntry::Attribute(value) => {
            json!({"kind":"attribute", "value":attribute_json(value)})
        }
        RequirementEntry::Entity(value) => json!({
            "kind":"entity", "id":value.id.get(), "name":value.name,
            "optional":value.optional, "assignment_sugar":value.assignment_sugar,
            "scope":value.scope.map(|id| id.0),
            "entries":value.entries.iter().map(requirement_json).collect::<Vec<_>>(),
            "poison":poison_json(&value.poison),
        }),
        RequirementEntry::Resource(value) => json!({
            "kind":"resource", "id":value.id.get(), "resource_kind":value.kind,
            "optional":value.optional, "key":value.key.as_ref().map(reference_json),
            "scope":value.scope.0,
            "entries":value.entries.iter().map(requirement_json).collect::<Vec<_>>(),
            "poison":poison_json(&value.poison),
        }),
        RequirementEntry::Extension(value) => json!({
            "kind":"extension", "id":value.id.get(), "namespace":value.namespace, "name":value.name,
            "scope":value.scope.0,
            "entries":value.entries.iter().map(requirement_json).collect::<Vec<_>>(),
            "poison":poison_json(&value.poison),
        }),
        RequirementEntry::Path(value) => json!({
            "kind":"path", "id":value.id.get(), "path":value.path, "optional":value.optional,
            "scope":value.scope.0,
            "entries":value.entries.iter().map(requirement_json).collect::<Vec<_>>(),
            "poison":poison_json(&value.poison),
        }),
        RequirementEntry::Poison(value) => json!({
            "kind":"poison", "id":value.id.get(), "range":range_json(value.range),
            "poison":poison_json(&value.poison),
        }),
    }
}

fn attribute_json(value: &Attribute) -> JsonValue {
    json!({
        "id": value.id.get(), "name": value.kind.as_str(), "value": value_json(&value.value),
        "poison": poison_json(&value.poison),
    })
}

fn field_json(value: &NamedField) -> JsonValue {
    json!({
        "id": value.id.get(), "name": value.name, "value": value_json(&value.value),
        "poison": poison_json(&value.poison),
    })
}

fn value_json(value: &HirValue) -> JsonValue {
    let kind = match &value.kind {
        HirValueKind::String(expression) => json!({
            "kind":"string", "value":expression.evaluated.value,
            "poisoned":expression.evaluated.poisoned,
            "segments":expression.segments.iter().map(expression_segment_json).collect::<Vec<_>>(),
            "evaluated_segments":expression.evaluated.segments.iter().map(evaluated_segment_json).collect::<Vec<_>>(),
        }),
        HirValueKind::Reference(reference) => json!({
            "kind":"reference", "name":reference.name, "namespace":reference.namespace.as_str(),
        }),
        HirValueKind::List(values) => json!({
            "kind":"list", "values":values.iter().map(value_json).collect::<Vec<_>>(),
        }),
        HirValueKind::Missing => json!({"kind":"missing"}),
    };
    json!({
        "id":value.id.get(), "type":value.value_type.as_str(), "kind":kind,
        "range":range_json(value.range), "poison":poison_json(&value.poison),
    })
}

fn reference_json(value: &ReferenceValue) -> JsonValue {
    json!({"name":value.name, "namespace":value.namespace.as_str(), "range":range_json(value.range)})
}

fn binding_json(value: &BindingDeclaration) -> JsonValue {
    json!({
        "id":value.id.0, "hir":value.hir_id.get(), "scope":value.scope.0,
        "name":value.name, "range":range_json(value.range), "used":value.used,
        "initializer":value.initializer.segments.iter().map(expression_segment_json).collect::<Vec<_>>(),
        "evaluated":{
            "value":value.evaluated.value, "poisoned":value.evaluated.poisoned,
            "segments":value.evaluated.segments.iter().map(evaluated_segment_json).collect::<Vec<_>>(),
        },
        "poison":poison_json(&value.poison),
    })
}

fn expression_segment_json(value: &ExpressionSegment) -> JsonValue {
    match value {
        ExpressionSegment::Literal { text, range } => {
            json!({"kind":"literal", "text":text, "range":range_json(*range)})
        }
        ExpressionSegment::Binding {
            name,
            range,
            resolution,
        } => json!({
            "kind":"binding", "name":name, "range":range_json(*range),
            "resolution":format!("{resolution:?}"),
        }),
        ExpressionSegment::Poison { range } => {
            json!({"kind":"poison", "range":range_json(*range)})
        }
    }
}

fn evaluated_segment_json(value: &EvaluatedSegment) -> JsonValue {
    json!({
        "text":value.text, "source_range":range_json(value.source_range),
        "binding_edges":value.binding_edges.iter().map(|edge| json!({
            "binding":edge.binding.0,
            "declaration_range":range_json(edge.declaration_range),
            "reference_range":range_json(edge.reference_range),
        })).collect::<Vec<_>>(),
    })
}

fn poison_json(values: &[Poison]) -> JsonValue {
    JsonValue::Array(
        values
            .iter()
            .map(|poison| json!({"kind":poison.kind.as_str(), "range":range_json(poison.range)}))
            .collect(),
    )
}

fn range_json(range: ByteRange) -> JsonValue {
    json!({"start":range.start(), "end":range.end()})
}
