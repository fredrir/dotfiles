//! Server-side view of the `dotfmt` configuration section.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixesOnSave {
    #[default]
    Off,
    Safe,
    Unsafe,
}

impl FixesOnSave {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "safe" => Some(Self::Safe),
            "unsafe" => Some(Self::Unsafe),
            _ => None,
        }
    }
}

/// Which TypeScript files receive `tsc` diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TscScope {
    /// Publish compiler errors only for documents open in the editor.
    #[default]
    Open,
    /// Publish compiler errors for every file in the project.
    Project,
}

impl TscScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "project" => Some(Self::Project),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Project => "project",
        }
    }
}

pub const DEBOUNCE_MIN_MS: u64 = 50;
pub const DEBOUNCE_MAX_MS: u64 = 2000;
pub const DEBOUNCE_DEFAULT_MS: u64 = 250;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub trusted: bool,
    pub type_aware: bool,
    pub syntax_diagnostics: bool,
    pub tsc_scope: TscScope,
    pub debounce_ms: u64,
    pub fixes_on_save: FixesOnSave,
    /// Explicit TypeScript SDK: a package directory, its `lib` directory, or `tsc.js`.
    pub tsdk: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trusted: false,
            type_aware: true,
            syntax_diagnostics: true,
            tsc_scope: TscScope::Open,
            debounce_ms: DEBOUNCE_DEFAULT_MS,
            fixes_on_save: FixesOnSave::Off,
            tsdk: None,
        }
    }
}

impl Settings {
    /// Applies either a whole `dotfmt` object or its parent settings object.
    ///
    /// Both nested (`{"dotfmt": {"fixes": {"onSave": "safe"}}}`) and flat
    /// dotted keys (`"dotfmt.fixes.onSave"`) are accepted. Trust is only ever
    /// enabled by an explicit `true`; absent values keep the current setting.
    /// `initializationOptions.tsdk` is accepted as well as
    /// `dotfmt.typescript.tsdk`.
    pub fn apply(&mut self, value: &Value) {
        let value = value
            .get("dotfmt")
            .filter(|value| value.is_object())
            .unwrap_or(value);
        if let Some(trusted) = lookup(value, &["trusted"]).and_then(Value::as_bool) {
            self.trusted = trusted;
        }
        if let Some(type_aware) = lookup(value, &["typeAware"]).and_then(Value::as_bool) {
            self.type_aware = type_aware;
        }
        if let Some(syntax) = lookup(value, &["diagnostics", "syntax"]).and_then(Value::as_bool) {
            self.syntax_diagnostics = syntax;
        }
        if let Some(scope) = lookup(value, &["diagnostics", "tscScope"]).and_then(Value::as_str)
            && let Some(scope) = TscScope::parse(scope)
        {
            self.tsc_scope = scope;
        }
        if let Some(debounce) = lookup(value, &["diagnostics", "debounceMs"]).and_then(Value::as_u64)
        {
            self.debounce_ms = debounce.clamp(DEBOUNCE_MIN_MS, DEBOUNCE_MAX_MS);
        }
        if let Some(mode) = lookup(value, &["fixes", "onSave"]).and_then(Value::as_str)
            && let Some(mode) = FixesOnSave::parse(mode)
        {
            self.fixes_on_save = mode;
        }
        let tsdk = lookup(value, &["typescript", "tsdk"])
            .or_else(|| value.get("tsdk"))
            .and_then(Value::as_str);
        if let Some(tsdk) = tsdk {
            self.tsdk = (!tsdk.is_empty()).then(|| PathBuf::from(tsdk));
        }
    }

    /// Enables trust when the server is started with `DOTFMT_TRUSTED=1`.
    pub fn apply_environment(&mut self) {
        if std::env::var("DOTFMT_TRUSTED").is_ok_and(|value| value == "1") {
            self.trusted = true;
        }
    }
}

fn lookup<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let dotted = path.join(".");
    let nested = path
        .iter()
        .try_fold(value, |current, key| current.get(*key));
    nested
        .or_else(|| value.get(dotted.as_str()))
        .or_else(|| value.get(format!("dotfmt.{dotted}").as_str()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_to_untrusted_and_errors_only() {
        let settings = Settings::default();
        assert!(!settings.trusted);
        assert!(settings.type_aware);
        assert!(settings.syntax_diagnostics);
        assert_eq!(settings.tsc_scope, TscScope::Open);
        assert_eq!(settings.debounce_ms, DEBOUNCE_DEFAULT_MS);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Off);
        assert_eq!(settings.tsdk, None);
    }

    #[test]
    fn reads_the_dotfmt_section_and_leaves_absent_values_alone() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
                "trusted": true,
                "typeAware": false,
                "diagnostics": {
                    "syntax": false,
                    "tscScope": "project",
                    "debounceMs": 500
                },
                "typescript": { "tsdk": "/opt/ts" },
                "fixes": { "onSave": "safe" }
            }
        }));
        assert!(settings.trusted);
        assert!(!settings.type_aware);
        assert!(!settings.syntax_diagnostics);
        assert_eq!(settings.tsc_scope, TscScope::Project);
        assert_eq!(settings.debounce_ms, 500);
        assert_eq!(settings.tsdk, Some(PathBuf::from("/opt/ts")));
        assert_eq!(settings.fixes_on_save, FixesOnSave::Safe);
        settings.apply(&json!({ "dotfmt": { "fixes": { "onSave": "unsafe" } } }));
        assert!(settings.trusted);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Unsafe);
        settings.apply(&json!({ "dotfmt": { "fixes": { "onSave": "nonsense" } } }));
        assert_eq!(settings.fixes_on_save, FixesOnSave::Unsafe);
    }

    #[test]
    fn reads_flat_dotted_keys() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt.typeAware": false,
            "dotfmt.fixes.onSave": "safe",
            "dotfmt.trusted": true,
            "dotfmt.diagnostics.syntax": false,
            "dotfmt.diagnostics.tscScope": "project",
            "dotfmt.diagnostics.debounceMs": 75,
            "dotfmt.typescript.tsdk": "/sdk"
        }));
        assert!(settings.trusted);
        assert!(!settings.type_aware);
        assert!(!settings.syntax_diagnostics);
        assert_eq!(settings.tsc_scope, TscScope::Project);
        assert_eq!(settings.debounce_ms, 75);
        assert_eq!(settings.tsdk, Some(PathBuf::from("/sdk")));
        assert_eq!(settings.fixes_on_save, FixesOnSave::Safe);
    }

    #[test]
    fn reads_flat_keys_inside_the_dotfmt_section() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
                "typeAware": false,
                "fixes.onSave": "unsafe",
                "diagnostics.tscScope": "project"
            }
        }));
        assert!(!settings.type_aware);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Unsafe);
        assert_eq!(settings.tsc_scope, TscScope::Project);
    }

    #[test]
    fn initialisation_options_accept_a_top_level_tsdk() {
        let mut settings = Settings::default();
        settings.apply(&json!({ "trusted": true, "tsdk": "/sdk" }));
        assert!(settings.trusted);
        assert_eq!(settings.tsdk, Some(PathBuf::from("/sdk")));
        settings.apply(&json!({ "tsdk": "" }));
        assert_eq!(settings.tsdk, None);
    }

    #[test]
    fn debounce_is_clamped() {
        let mut settings = Settings::default();
        settings.apply(&json!({ "dotfmt.diagnostics.debounceMs": 1 }));
        assert_eq!(settings.debounce_ms, DEBOUNCE_MIN_MS);
        settings.apply(&json!({ "dotfmt.diagnostics.debounceMs": 99_999 }));
        assert_eq!(settings.debounce_ms, DEBOUNCE_MAX_MS);
    }

    #[test]
    fn an_explicit_false_can_revoke_trust() {
        let mut settings = Settings::default();
        settings.apply(&json!({ "dotfmt": { "trusted": true } }));
        assert!(settings.trusted);
        settings.apply(&json!({ "dotfmt": { "trusted": false } }));
        assert!(!settings.trusted);
    }
}
