use super::{
    Result,
    color::Color,
    expression::{Expr, Resolved},
};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};
pub const ANSI: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];
pub const UI: [&str; 5] = ["background", "primary", "accent", "surface", "foreground"];
pub fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
pub fn table(value: &Value) -> Result<&serde_json::Map<String, Value>> {
    value.as_object().ok_or_else(|| "expected table".into())
}
pub fn load(path: &Path) -> Result<Value> {
    let source = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: toml::Value =
        toml::from_str(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::to_value(value).map_err(|e| e.to_string())
}
pub struct Data {
    pub roles: Value,
    pub fonts: Value,
    pub maps: BTreeMap<String, Value>,
    pub yazi: String,
    pub contracts: Value,
    expressions: RefCell<HashMap<String, Expr>>,
}
pub struct Repository {
    pub root: PathBuf,
    pub root_config: PathBuf,
    pub data: Rc<Data>,
    pub themes: BTreeMap<String, Theme>,
}
#[derive(Eq, PartialEq, Hash)]
struct ContrastKey {
    seed: Color,
    backgrounds: Vec<Color>,
    floor: u64,
}
pub struct Theme {
    pub kde: RefCell<Option<Rc<Vec<super::resolve::KdeGroup>>>>,
    pub ui: RefCell<Option<Rc<ui_theme::PaletteDocument>>>,
    pub profile: String,
    pub name: String,
    pub dark: bool,
    pub raw: Value,
    pub data: Rc<Data>,
    primitives: HashMap<String, Color>,
    resolved: RefCell<HashMap<String, Resolved>>,
    many: RefCell<HashMap<ContrastKey, Color>>,
}
impl Repository {
    pub fn load(root: &Path, root_config: &Path) -> Result<Self> {
        let directory = root.join("theme");
        let mut maps = BTreeMap::new();
        for entry in fs::read_dir(directory.join("maps")).map_err(|e| format!("theme/maps: {e}"))? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p.extension().is_some_and(|x| x == "toml") {
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("invalid map name")?
                    .to_string();
                maps.insert(name, load(&p)?);
            }
        }
        let data = Rc::new(Data {
            roles: load(&directory.join("roles.toml"))?,
            fonts: load(&directory.join("fonts.toml"))?,
            maps,
            yazi: fs::read_to_string(directory.join("maps/yazi.toml"))
                .map_err(|e| e.to_string())?,
            contracts: serde_json::from_str(include_str!("contracts.json"))
                .map_err(|e| e.to_string())?,
            expressions: RefCell::new(HashMap::new()),
        });
        let mut themes = BTreeMap::new();
        for name in profile_names(root)? {
            let raw = load(&directory.join("profiles").join(format!("{name}.toml")))?;
            themes.insert(name.clone(), Theme::new(&name, raw, data.clone())?);
        }
        Ok(Self {
            root: root.into(),
            root_config: root_config.into(),
            data,
            themes,
        })
    }
    pub fn theme(&self, name: &str) -> Result<&Theme> {
        self.themes.get(name).ok_or_else(|| {
            format!(
                "unknown profile '{name}' (available: {})",
                self.names().join(", ")
            )
        })
    }
    pub fn names(&self) -> Vec<String> {
        self.themes.keys().cloned().collect()
    }
}
pub fn profile_names(root: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in
        fs::read_dir(root.join("theme/profiles")).map_err(|e| format!("theme/profiles: {e}"))?
    {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|x| x == "toml") {
            names.push(
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("invalid profile name")?
                    .to_string(),
            );
        }
    }
    names.sort();
    Ok(names)
}
impl Theme {
    pub(super) fn new(profile: &str, mut raw: Value, data: Rc<Data>) -> Result<Self> {
        let mut errors = Vec::new();
        keys(&raw, &["name", "dark", "ui", "ansi"], profile, &mut errors);
        let name = text(&raw["name"]).trim().to_string();
        if name.is_empty() {
            errors.push("name must be a non-empty string".into());
        }
        let dark = raw["dark"].as_bool().unwrap_or_else(|| {
            errors.push("dark must be true or false".into());
            false
        });
        keys(&raw["ui"], &UI, "ui", &mut errors);
        keys(&raw["ansi"], &["normal", "bright"], "ansi", &mut errors);
        let mut primitives = HashMap::new();
        for (prefix, value, names) in [
            ("ui", &raw["ui"], UI.as_slice()),
            ("ansi.normal", &raw["ansi"]["normal"], ANSI.as_slice()),
            ("ansi.bright", &raw["ansi"]["bright"], ANSI.as_slice()),
        ] {
            keys(value, names, prefix, &mut errors);
            for key in names {
                let s = text(&value[*key]);
                match if s.starts_with('#') {
                    Color::parse(s)
                } else {
                    Err(format!("invalid color: {s}"))
                } {
                    Ok(c) => {
                        primitives.insert(format!("{prefix}.{key}"), c);
                    }
                    Err(_) => errors.push(format!("{prefix}.{key}: must be a six-digit hex color")),
                }
            }
        }
        if !errors.is_empty() {
            return Err(format!(
                "profile '{profile}' is not usable:\n  {}",
                errors.join("\n  ")
            ));
        }
        raw["name"] = name.clone().into();
        for key in UI {
            raw["ui"][key] = primitives[&format!("ui.{key}")].to_string().into();
        }
        for group in ["normal", "bright"] {
            for key in ANSI {
                raw["ansi"][group][key] = primitives[&format!("ansi.{group}.{key}")]
                    .to_string()
                    .into();
            }
        }
        Ok(Self {
            kde: RefCell::new(None),
            ui: RefCell::new(None),
            profile: profile.into(),
            name,
            dark,
            raw,
            data,
            primitives,
            resolved: RefCell::new(HashMap::new()),
            many: RefCell::new(HashMap::new()),
        })
    }
    pub fn resolve(&self, expression: &str) -> Result<Resolved> {
        self.resolve_stack(expression, &mut Vec::new())
    }
    fn resolve_stack(&self, expression: &str, stack: &mut Vec<String>) -> Result<Resolved> {
        if let Some(value) = self.resolved.borrow().get(expression).copied() {
            return Ok(value);
        }
        if stack.iter().any(|s| s == expression) {
            return Err(format!(
                "semantic color cycle: {} -> {expression}",
                stack.join(" -> ")
            ));
        }
        if stack.len() > 128 {
            return Err("semantic color nesting exceeds 128".into());
        }
        let parsed = { self.data.expressions.borrow().get(expression).cloned() };
        let expr = match parsed {
            Some(expr) => expr,
            None => {
                let expr = Expr::parse(expression)?;
                self.data
                    .expressions
                    .borrow_mut()
                    .insert(expression.into(), expr.clone());
                expr
            }
        };
        stack.push(expression.into());
        let value = expr.evaluate(
            &mut |name| {
                if let Some(c) = self.primitives.get(name) {
                    return Ok(*c);
                }
                let expression = self.data.contracts["semantics"][name]
                    .as_str()
                    .ok_or_else(|| format!("unknown palette color: {name}"))?;
                Ok(self.resolve_stack(expression, stack)?.color)
            },
            self.primitives["ui.background"],
            self.primitives["ui.foreground"],
        );
        stack.pop();
        let value = value?;
        self.resolved.borrow_mut().insert(expression.into(), value);
        Ok(value)
    }
    pub fn color(&self, expression: &str) -> Result<Color> {
        Ok(self.resolve(expression)?.color)
    }
    pub fn role(&self, name: &str) -> Result<Color> {
        self.app("roles", name)
    }
    pub fn app(&self, app: &str, name: &str) -> Result<Color> {
        let expr = self.data.roles[app][name]
            .as_str()
            .ok_or_else(|| format!("unknown {app} role: {name}"))?;
        self.color(expr)
    }
    pub fn mapped(&self, app: &str, name: &str) -> Result<Color> {
        self.color(self.data.roles[app][name].as_str().unwrap_or(name))
    }
    pub fn css(&self, expression: &str) -> Result<String> {
        let r = self.resolve(expression)?;
        Ok(if let Some(alpha) = r.alpha {
            let [red, green, blue] = r.color.0;
            format!("rgba({red}, {green}, {blue}, {alpha})")
        } else {
            r.color.to_string()
        })
    }
    pub fn readable_many(
        &self,
        expression: &str,
        backgrounds: &[Color],
        floor: f64,
    ) -> Result<Color> {
        let seed = self.color(expression)?;
        let key = ContrastKey {
            seed,
            backgrounds: backgrounds.to_vec(),
            floor: floor.to_bits(),
        };
        if let Some(c) = self.many.borrow().get(&key) {
            return Ok(*c);
        }
        let c = seed.readable_many(backgrounds, floor)?;
        self.many.borrow_mut().insert(key, c);
        Ok(c)
    }
    pub fn font(&self, name: &str) -> Result<&str> {
        self.data.fonts["fonts"][name]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("unknown font role: {name}"))
    }
    pub fn size(&self, name: &str) -> Result<String> {
        let v = &self.data.fonts["sizes"][name];
        if !v.is_number() {
            return Err(format!("unknown font size: {name}"));
        }
        Ok(v.to_string())
    }
    pub fn uses_fonts(&self, app: &str) -> Result<bool> {
        match self.data.fonts["applications"].get(app) {
            None => Ok(false),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| format!("font application setting must be true or false: {app}")),
        }
    }
    pub fn header(&self) -> String {
        self.profile.to_string()
    }
    pub fn icons(&self) -> &str {
        if self.dark {
            "Breeze Chameleon Dark"
        } else {
            "breeze"
        }
    }
    pub fn palette_names() -> Vec<String> {
        UI.into_iter()
            .map(str::to_string)
            .chain(ANSI.into_iter().map(str::to_string))
            .chain(ANSI.into_iter().map(|s| format!("bright_{s}")))
            .collect()
    }
    pub fn map(&self, name: &str) -> Result<&Value> {
        self.data
            .maps
            .get(name)
            .ok_or_else(|| format!("missing map: {name}"))
    }
}
fn keys(value: &Value, expected: &[&str], where_: &str, errors: &mut Vec<String>) {
    let Some(table) = value.as_object() else {
        errors.push(format!("{where_}: must be a table"));
        return;
    };
    for name in expected {
        if !table.contains_key(*name) {
            errors.push(format!("{where_}: missing '{name}'"));
        }
    }
    for name in table.keys() {
        if !expected.contains(&name.as_str()) {
            errors.push(format!("{where_}: unknown '{name}'"));
        }
    }
}
