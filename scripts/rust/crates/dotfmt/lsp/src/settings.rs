//! Server-side view of the `dotfmt` configuration section.

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub trusted: bool,
    pub type_aware: bool,
    pub fixes_on_save: FixesOnSave,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trusted: false,
            type_aware: true,
            fixes_on_save: FixesOnSave::Off,
        }
    }
}

impl Settings {
    /// Applies either a whole `dotfmt` object or its parent settings object.
    ///
    /// Both nested (`{"dotfmt": {"fixes": {"onSave": "safe"}}}`) and flat
    /// dotted keys (`"dotfmt.fixes.onSave"`) are accepted. Trust is only ever
    /// enabled by an explicit `true`; absent values keep the current setting.
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
        if let Some(mode) = lookup(value, &["fixes", "onSave"]).and_then(Value::as_str)
            && let Some(mode) = FixesOnSave::parse(mode)
        {
            self.fixes_on_save = mode;
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
    fn defaults_to_untrusted() {
        assert!(!Settings::default().trusted);
        assert!(Settings::default().type_aware);
        assert_eq!(Settings::default().fixes_on_save, FixesOnSave::Off);
    }

    #[test]
    fn reads_the_dotfmt_section_and_leaves_absent_values_alone() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
                "trusted": true,
                "typeAware": false,
                "fixes": { "onSave": "safe" }
            }
        }));
        assert!(settings.trusted);
        assert!(!settings.type_aware);
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
            "dotfmt.trusted": true
        }));
        assert!(settings.trusted);
        assert!(!settings.type_aware);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Safe);
    }

    #[test]
    fn reads_flat_keys_inside_the_dotfmt_section() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
                "typeAware": false,
                "fixes.onSave": "unsafe"
            }
        }));
        assert!(!settings.type_aware);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Unsafe);
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
