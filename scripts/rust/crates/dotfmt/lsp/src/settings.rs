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

#[derive(Clone, Debug)]
pub struct Settings {
    pub trusted: bool,
    pub type_aware: bool,
    pub fixes_on_save: FixesOnSave,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trusted: true,
            type_aware: true,
            fixes_on_save: FixesOnSave::Off,
        }
    }
}

impl Settings {
    /// Applies either a whole `dotfmt` object or its parent settings object.
    pub fn apply(&mut self, value: &Value) {
        let value = value.get("dotfmt").unwrap_or(value);
        if let Some(trusted) = value.get("trusted").and_then(Value::as_bool) {
            self.trusted = trusted;
        }
        if let Some(type_aware) = value.get("typeAware").and_then(Value::as_bool) {
            self.type_aware = type_aware;
        }
        if let Some(fixes) = value.get("fixes").and_then(Value::as_object)
            && let Some(mode) = fixes.get("onSave").and_then(Value::as_str)
            && let Some(mode) = FixesOnSave::parse(mode)
        {
            self.fixes_on_save = mode;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_the_dotfmt_section_and_leaves_absent_values_alone() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
                "trusted": false,
                "typeAware": false,
                "fixes": { "onSave": "safe" }
            }
        }));
        assert!(!settings.trusted);
        assert!(!settings.type_aware);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Safe);
        settings.apply(&json!({ "dotfmt": { "fixes": { "onSave": "unsafe" } } }));
        assert!(!settings.trusted);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Unsafe);
        settings.apply(&json!({ "dotfmt": { "fixes": { "onSave": "nonsense" } } }));
        assert_eq!(settings.fixes_on_save, FixesOnSave::Unsafe);
    }
}
