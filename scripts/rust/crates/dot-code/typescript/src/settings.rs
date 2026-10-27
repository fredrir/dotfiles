use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub extract: Extract,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Extract {
    /// `${name}` expands to the enclosing function, class or variable name.
    pub name: String,
    pub export: Export,
    pub rename: bool,
}

impl Default for Extract {
    fn default() -> Self {
        Self {
            name: "Props".into(),
            export: Export::Never,
            rename: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Export {
    #[default]
    Never,
    Always,
    /// Export when the statement the type is extracted from is exported.
    Inherit,
}
