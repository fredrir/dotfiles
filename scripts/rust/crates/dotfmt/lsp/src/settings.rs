//! Server-side view of the `dotfmt` configuration section.
//!
//! Editor settings are resource scoped: every workspace folder can have its
//! own `typeAware`, `diagnostics.*`, `fixes.onSave` and `typescript.tsdk`
//! values. Trust is the exception: it belongs to the workspace window, so it
//! is stored once per process and never changes with a folder configuration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use dotfmt_core::config::ConfigOverlay;
use lsp_types::Uri;
use serde_json::{Map, Value};

use crate::uri;
use crate::workspace::normalize;

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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Safe => "safe",
            Self::Unsafe => "unsafe",
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

/// One resolved `dotfmt` configuration section.
///
/// Trust is deliberately absent: it is process level and lives in
/// [`SettingsStore`]. A folder configuration can never change it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub type_aware: bool,
    pub syntax_diagnostics: bool,
    pub tsc_scope: TscScope,
    pub debounce_ms: u64,
    pub fixes_on_save: FixesOnSave,
    /// Explicit TypeScript SDK: a package directory, its `lib` directory, or `tsc.js`.
    pub tsdk: Option<PathBuf>,
    /// Native `dotfmt.format` overlay, normalized to the nested dotfmt.dotfile shape.
    pub format: Option<Value>,
    /// Free-form `dotfmt.formatExtra`; lower precedence than [`Settings::format`].
    pub format_extra: Option<Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            type_aware: true,
            syntax_diagnostics: true,
            tsc_scope: TscScope::Open,
            debounce_ms: DEBOUNCE_DEFAULT_MS,
            fixes_on_save: FixesOnSave::Off,
            tsdk: None,
            format: None,
            format_extra: None,
        }
    }
}

impl Settings {
    /// Applies either a whole `dotfmt` object or its parent settings object.
    ///
    /// Both nested (`{"dotfmt": {"fixes": {"onSave": "safe"}}}`) and flat
    /// dotted keys (`"dotfmt.fixes.onSave"`) are accepted. Absent values keep
    /// the current setting. Trust is read by [`Settings::trust`] instead and
    /// is never applied to a folder configuration.
    pub fn apply(&mut self, value: &Value) {
        let value = value
            .get("dotfmt")
            .filter(|value| value.is_object())
            .unwrap_or(value);
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
        if let Some(debounce) =
            lookup(value, &["diagnostics", "debounceMs"]).and_then(Value::as_u64)
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
            self.tsdk = (!tsdk.is_empty()).then(|| expand_home(tsdk));
        }
        self.apply_overlays(value);
    }

    /// Reads `format`/`formatExtra` and folds flat `format.*` keys into the
    /// nested object. `null` clears an overlay so the setting inherits again.
    fn apply_overlays(&mut self, value: &Value) {
        if let Some(format) = overlay_setting(value, "format") {
            self.format = match format {
                Value::Null => None,
                other => Some(other.clone()),
            };
        }
        if let Some(extra) =
            overlay_setting(value, "formatExtra").or_else(|| overlay_setting(value, "format_extra"))
        {
            self.format_extra = match extra {
                Value::Null => None,
                other => Some(other.clone()),
            };
        }
        let Some(object) = value.as_object() else {
            return;
        };
        let mut flat: Vec<(&str, Option<&Value>)> = Vec::new();
        for (key, item) in object {
            let path = key
                .strip_prefix("format.")
                .or_else(|| key.strip_prefix("dotfmt.format."));
            if let Some(path) = path.filter(|path| !path.is_empty()) {
                flat.push((path, (!item.is_null()).then_some(item)));
            }
        }
        if flat.is_empty() {
            return;
        }
        let mut root = match self.format.take() {
            Some(Value::Object(object)) => object,
            _ => Map::new(),
        };
        for (path, item) in flat {
            let parts: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
            match item {
                Some(item) => set_path(&mut root, &parts, item.clone()),
                None => remove_path(&mut root, &parts),
            }
        }
        self.format = Some(Value::Object(root));
    }

    /// A fresh configuration from a pull response or an initialization option.
    #[allow(dead_code)]
    pub fn from_value(value: &Value) -> Self {
        let mut settings = Self::default();
        settings.apply(value);
        settings
    }

    /// The explicit `trusted` value in an initialization option or settings object.
    pub fn trust(value: &Value) -> Option<bool> {
        let value = value
            .get("dotfmt")
            .filter(|value| value.is_object())
            .unwrap_or(value);
        lookup(value, &["trusted"]).and_then(Value::as_bool)
    }
}

/// The process-wide trust flag plus one configuration per workspace folder.
pub struct SettingsStore {
    trusted: AtomicBool,
    default: RwLock<Settings>,
    folders: Mutex<HashMap<PathBuf, Settings>>,
    default_overlay: RwLock<Option<Arc<ConfigOverlay>>>,
    folder_overlays: Mutex<HashMap<PathBuf, Arc<ConfigOverlay>>>,
}

impl SettingsStore {
    pub fn new(trusted: bool, default: Settings) -> Self {
        Self {
            trusted: AtomicBool::new(trusted),
            default: RwLock::new(default),
            folders: Mutex::new(HashMap::new()),
            default_overlay: RwLock::new(None),
            folder_overlays: Mutex::new(HashMap::new()),
        }
    }

    /// Enables trust when the server is started with `DOTFMT_TRUSTED=1`.
    pub fn trust_from_environment() -> Option<bool> {
        std::env::var("DOTFMT_TRUSTED").ok().map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
    }

    pub fn trusted(&self) -> bool {
        self.trusted.load(Ordering::SeqCst)
    }

    pub fn set_trusted(&self, trusted: bool) -> bool {
        self.trusted.swap(trusted, Ordering::SeqCst) != trusted
    }

    pub fn default_settings(&self) -> Settings {
        self.default
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The nearest folder configuration containing `path`, or the default.
    ///
    /// A path outside every workspace folder uses the default; it must never
    /// inherit the first folder's settings.
    pub fn settings_for(&self, path: &Path) -> Settings {
        let path = normalize(path);
        let folders = self
            .folders
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        folders
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, settings)| settings.clone())
            .unwrap_or_else(|| self.default_settings())
    }

    pub fn settings_for_uri(&self, uri: &Uri) -> Settings {
        uri::path(uri).map_or_else(|| self.default_settings(), |path| self.settings_for(&path))
    }

    pub fn settings_for_root(&self, root: &Path) -> Settings {
        let root = normalize(root);
        let folders = self
            .folders
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        folders
            .get(&root)
            .cloned()
            .unwrap_or_else(|| self.default_settings())
    }

    pub fn has_folder(&self, root: &Path) -> bool {
        let root = normalize(root);
        self.folders
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&root)
    }

    /// Replaces the validated `dotfmt.format` overlay used outside a folder.
    pub fn set_default_overlay(&self, overlay: Option<Arc<ConfigOverlay>>) {
        *self
            .default_overlay
            .write()
            .unwrap_or_else(|error| error.into_inner()) = overlay;
    }

    /// Replaces one folder's validated overlay; the default still applies elsewhere.
    pub fn set_folder_overlay(&self, root: PathBuf, overlay: Arc<ConfigOverlay>) {
        let root = normalize(&root);
        self.folder_overlays
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(root, overlay);
    }

    /// Drops one folder overlay so the folder inherits the default overlay again.
    pub fn remove_folder_overlay(&self, root: &Path) {
        let root = normalize(root);
        self.folder_overlays
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&root);
    }

    /// The native overlay for `path`: the nearest folder's, or the default.
    pub fn overlay_for(&self, path: &Path) -> Option<Arc<ConfigOverlay>> {
        let path = normalize(path);
        let root = {
            let folders = self
                .folder_overlays
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            folders
                .keys()
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.components().count())
                .cloned()
        };
        match root {
            Some(root) => self.overlay_for_root(&root),
            None => self.default_overlay(),
        }
    }

    /// The overlay for an exact workspace root, falling back to the default.
    pub fn overlay_for_root(&self, root: &Path) -> Option<Arc<ConfigOverlay>> {
        let root = normalize(root);
        self.folder_overlays
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&root)
            .map(Arc::clone)
            .or_else(|| self.default_overlay())
    }

    fn default_overlay(&self) -> Option<Arc<ConfigOverlay>> {
        self.default_overlay
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn set_folder(&self, root: PathBuf, settings: Settings) {
        let root = normalize(&root);
        self.folders
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(root, settings);
    }

    pub fn remove_folder(&self, root: &Path) {
        let root = normalize(root);
        self.folders
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&root);
        self.remove_folder_overlay(&root);
    }

    /// Replaces the default from a full pull response; returns whether it changed.
    pub fn configure_default(&self, value: &Value) -> bool {
        if value.is_null() {
            return false;
        }
        let mut default = self
            .default
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let before = default.clone();
        // A full pull omits unset generated settings; absent must clear the
        // overlay so removing a VS Code setting stops overriding dotfmt.dotfile.
        default.format = None;
        default.format_extra = None;
        default.apply(value);
        *default != before
    }

    /// Applies a partial `didChangeConfiguration` payload to the default.
    pub fn apply_default(&self, value: &Value) -> bool {
        let mut default = self
            .default
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let before = default.clone();
        default.apply(value);
        *default != before
    }

    /// Replaces one folder configuration; returns whether it changed.
    pub fn configure_folder(&self, root: &Path, value: &Value) -> bool {
        if value.is_null() {
            return false;
        }
        let root = normalize(root);
        let mut settings = self.settings_for_root(&root);
        // A folder pull is fully resolved: its own native overlay replaces the
        // old one, and an absent `format` falls back to the default overlay.
        settings.format = None;
        settings.format_extra = None;
        settings.apply(value);
        let mut folders = self
            .folders
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let changed = folders.get(&root) != Some(&settings);
        folders.insert(root, settings);
        changed
    }
}

fn overlay_setting<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value
        .get(name)
        .or_else(|| value.get(format!("dotfmt.{name}").as_str()))
}

fn set_path(root: &mut Map<String, Value>, parts: &[&str], value: Value) {
    let Some((last, parents)) = parts.split_last() else {
        return;
    };
    let mut current = root;
    for part in parents {
        let entry = current
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry.as_object_mut().expect("object entry");
    }
    current.insert((*last).to_string(), value);
}

fn remove_path(root: &mut Map<String, Value>, parts: &[&str]) {
    let Some((last, parents)) = parts.split_last() else {
        return;
    };
    if parents.is_empty() {
        root.remove(*last);
        return;
    }
    if let Some(Value::Object(child)) = root.get_mut(parents[0]) {
        remove_path(child, &parts[1..]);
        if child.is_empty() {
            root.remove(parents[0]);
        }
    }
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(value)
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
    fn defaults_to_errors_only_settings() {
        let settings = Settings::default();
        assert!(settings.type_aware);
        assert!(settings.syntax_diagnostics);
        assert_eq!(settings.tsc_scope, TscScope::Open);
        assert_eq!(settings.debounce_ms, DEBOUNCE_DEFAULT_MS);
        assert_eq!(settings.fixes_on_save, FixesOnSave::Off);
        assert_eq!(settings.tsdk, None);
        assert_eq!(settings.format, None);
        assert_eq!(settings.format_extra, None);
    }

    #[test]
    fn reads_the_dotfmt_section_and_leaves_absent_values_alone() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
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
        assert!(!settings.type_aware);
        assert!(!settings.syntax_diagnostics);
        assert_eq!(settings.tsc_scope, TscScope::Project);
        assert_eq!(settings.debounce_ms, 500);
        assert_eq!(settings.tsdk, Some(PathBuf::from("/opt/ts")));
        assert_eq!(settings.fixes_on_save, FixesOnSave::Safe);
        settings.apply(&json!({ "dotfmt": { "fixes": { "onSave": "unsafe" } } }));
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
            "dotfmt.diagnostics.syntax": false,
            "dotfmt.diagnostics.tscScope": "project",
            "dotfmt.diagnostics.debounceMs": 75,
            "dotfmt.typescript.tsdk": "/sdk"
        }));
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
        settings.apply(&json!({ "tsdk": "/sdk" }));
        assert_eq!(settings.tsdk, Some(PathBuf::from("/sdk")));
        settings.apply(&json!({ "tsdk": "" }));
        assert_eq!(settings.tsdk, None);
    }

    #[test]
    fn trust_is_read_but_never_applied_to_settings() {
        assert_eq!(Settings::trust(&json!({ "trusted": true })), Some(true));
        assert_eq!(
            Settings::trust(&json!({ "dotfmt": { "trusted": true } })),
            Some(true)
        );
        assert_eq!(
            Settings::trust(&json!({ "dotfmt.trusted": false })),
            Some(false)
        );
        assert_eq!(Settings::trust(&json!({ "typeAware": false })), None);
        let mut settings = Settings::default();
        settings.apply(&json!({ "dotfmt": { "trusted": true } }));
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn reads_nested_and_flat_format_objects() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": {
                "format": {
                    "global": { "width": 100 },
                    "typescript": { "quote_style": "single" }
                },
                "format.typescript.quote_style": "double",
                "format.json.indent": 4
            }
        }));
        assert_eq!(
            settings.format,
            Some(json!({
                "global": { "width": 100 },
                "typescript": { "quote_style": "double" },
                "json": { "indent": 4 }
            }))
        );
        settings.apply(&json!({ "dotfmt.format.global.width": 120 }));
        assert_eq!(
            settings.format,
            Some(json!({
                "global": { "width": 120 },
                "typescript": { "quote_style": "double" },
                "json": { "indent": 4 }
            }))
        );
        settings.apply(&json!({ "dotfmt.format.global.width": null }));
        assert_eq!(
            settings.format,
            Some(json!({
                "typescript": { "quote_style": "double" },
                "json": { "indent": 4 }
            }))
        );
        settings.apply(&json!({ "dotfmt.format": null }));
        assert_eq!(settings.format, None);
    }

    #[test]
    fn reads_format_extra_and_ignores_null() {
        let mut settings = Settings::default();
        settings.apply(&json!({
            "dotfmt": { "formatExtra": { "global": { "repair": false } } }
        }));
        assert_eq!(
            settings.format_extra,
            Some(json!({ "global": { "repair": false } }))
        );
        settings.apply(&json!({ "dotfmt.formatExtra": null }));
        assert_eq!(settings.format_extra, None);
        settings.apply(&json!({ "dotfmt": { "format_extra": { "json": { "indent": 2 } } } }));
        assert_eq!(
            settings.format_extra,
            Some(json!({ "json": { "indent": 2 } }))
        );
    }

    #[test]
    fn overlays_fall_back_to_the_default_and_follow_the_nearest_folder() {
        let store = SettingsStore::new(false, Settings::default());
        let default = Arc::new(ConfigOverlay::from_json(&json!({}), Path::new("/")).unwrap());
        store.set_default_overlay(Some(Arc::clone(&default)));
        assert!(Arc::ptr_eq(
            &store.overlay_for(Path::new("/elsewhere/a.ts")).unwrap(),
            &default
        ));
        let outer = Arc::new(
            ConfigOverlay::from_json(&json!({ "global": { "width": 100 } }), Path::new("/work"))
                .unwrap(),
        );
        let inner = Arc::new(
            ConfigOverlay::from_json(
                &json!({ "global": { "width": 120 } }),
                Path::new("/work/project"),
            )
            .unwrap(),
        );
        store.set_folder_overlay(PathBuf::from("/work"), Arc::clone(&outer));
        store.set_folder_overlay(PathBuf::from("/work/project"), Arc::clone(&inner));
        assert!(Arc::ptr_eq(
            &store.overlay_for(Path::new("/work/a.ts")).unwrap(),
            &outer
        ));
        assert!(Arc::ptr_eq(
            &store
                .overlay_for(Path::new("/work/project/src/a.ts"))
                .unwrap(),
            &inner
        ));
        assert!(Arc::ptr_eq(
            &store.overlay_for_root(Path::new("/work/project")).unwrap(),
            &inner
        ));
        store.remove_folder_overlay(Path::new("/work/project"));
        assert!(Arc::ptr_eq(
            &store
                .overlay_for(Path::new("/work/project/src/a.ts"))
                .unwrap(),
            &outer
        ));
        store.remove_folder(Path::new("/work"));
        assert!(Arc::ptr_eq(
            &store.overlay_for(Path::new("/work/a.ts")).unwrap(),
            &default
        ));
    }

    #[test]
    fn removing_a_native_overlay_from_a_full_pull_clears_it() {
        let store = SettingsStore::new(false, Settings::default());
        assert!(store.configure_default(&json!({
            "dotfmt.format.global.width": 100
        })));
        assert!(store.default_settings().format.is_some());
        assert!(store.configure_default(&json!({})));
        assert_eq!(store.default_settings().format, None);

        store.set_folder(
            PathBuf::from("/work"),
            Settings::from_value(&json!({ "dotfmt.format.global.width": 80 })),
        );
        assert!(store.configure_folder(Path::new("/work"), &json!({})));
        assert_eq!(store.settings_for_root(Path::new("/work")).format, None);
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
    fn the_nearest_folder_wins_and_outside_paths_use_the_default() {
        let store = SettingsStore::new(false, Settings::default());
        let outer = Settings::from_value(&json!({ "dotfmt.typeAware": false }));
        let inner = Settings::from_value(&json!({ "dotfmt.diagnostics.syntax": false }));
        store.set_folder(PathBuf::from("/work"), outer.clone());
        store.set_folder(PathBuf::from("/work/project"), inner.clone());
        assert_eq!(store.settings_for(Path::new("/work/a.ts")), outer);
        assert_eq!(
            store.settings_for(Path::new("/work/project/src/a.ts")),
            inner
        );
        assert_eq!(
            store.settings_for(Path::new("/elsewhere/a.ts")),
            Settings::default()
        );
        assert_eq!(
            store.settings_for(Path::new("/work/other/nested/a.ts")),
            outer
        );
    }

    #[test]
    fn folder_configuration_never_changes_trust() {
        let store = SettingsStore::new(true, Settings::default());
        assert!(store.trusted());
        assert!(store.configure_folder(Path::new("/work"), &json!({ "dotfmt.trusted": false })));
        assert!(store.trusted());
        assert!(!store.configure_default(&json!({ "dotfmt.trusted": false })));
        assert!(store.trusted());
        assert!(store.set_trusted(false));
        assert!(!store.trusted());
    }

    #[test]
    fn null_pull_responses_keep_existing_settings() {
        let initial = Settings {
            tsdk: Some(PathBuf::from("/opt/typescript")),
            ..Settings::default()
        };
        let store = SettingsStore::new(true, initial);
        assert!(!store.configure_default(&Value::Null));
        assert_eq!(
            store.default_settings().tsdk,
            Some(PathBuf::from("/opt/typescript"))
        );
    }

    #[test]
    fn configuration_changes_compare_with_partial_eq() {
        let store = SettingsStore::new(false, Settings::default());
        assert!(store.configure_folder(Path::new("/work"), &json!({ "dotfmt.typeAware": false })));
        assert!(!store.configure_folder(Path::new("/work"), &json!({ "dotfmt.typeAware": false })));
        assert!(store.configure_folder(Path::new("/work"), &json!({ "dotfmt.typeAware": true })));
        assert!(store.configure_default(&json!({ "dotfmt.diagnostics.tscScope": "project" })));
        assert!(!store.configure_default(&json!({ "dotfmt.diagnostics.tscScope": "project" })));
    }

    #[test]
    fn removing_and_replacing_folders_updates_lookups() {
        let store = SettingsStore::new(false, Settings::default());
        store.set_folder(
            PathBuf::from("/work"),
            Settings::from_value(&json!({ "dotfmt.typeAware": false })),
        );
        assert!(store.has_folder(Path::new("/work")));
        store.remove_folder(Path::new("/work"));
        assert!(!store.has_folder(Path::new("/work")));
        assert_eq!(
            store.settings_for(Path::new("/work/a.ts")),
            Settings::default()
        );
    }
}
