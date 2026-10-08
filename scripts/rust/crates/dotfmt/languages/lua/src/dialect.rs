use std::path::Path;

use std::str::FromStr;
use stylua_lib::LuaVersion;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    All,
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Luajit,
    Luau,
}

impl Dialect {
    pub const ALL: [Self; 8] = [
        Self::Auto,
        Self::All,
        Self::Lua51,
        Self::Lua52,
        Self::Lua53,
        Self::Lua54,
        Self::Luajit,
        Self::Luau,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::All => "all",
            Self::Lua51 => "lua51",
            Self::Lua52 => "lua52",
            Self::Lua53 => "lua53",
            Self::Lua54 => "lua54",
            Self::Luajit => "luajit",
            Self::Luau => "luau",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::All => &[],
            Self::Lua51 => &[],
            Self::Lua52 => &[],
            Self::Lua53 => &[],
            Self::Lua54 => &[],
            Self::Luajit => &[],
            Self::Luau => &[],
        }
    }

    pub fn parse(value: &str, ignore_case: bool) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|dialect| {
                std::iter::once(dialect.name())
                    .chain(dialect.aliases().iter().copied())
                    .any(|name| {
                        if ignore_case {
                            name.eq_ignore_ascii_case(value)
                        } else {
                            name == value
                        }
                    })
            })
            .ok_or_else(|| format!("invalid variant: {value}"))
    }

    pub fn resolve(self, configured: Self, path: &Path) -> Self {
        let selected = if self == Self::Auto { configured } else { self };
        if selected != Self::Auto {
            return selected;
        }
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("luau"))
        {
            Self::Luau
        } else {
            Self::Lua54
        }
    }

    pub fn syntax(self) -> LuaVersion {
        match self {
            Self::Auto => LuaVersion::Lua54,
            Self::All => LuaVersion::All,
            Self::Lua51 => LuaVersion::Lua51,
            Self::Lua52 => LuaVersion::Lua52,
            Self::Lua53 => LuaVersion::Lua53,
            Self::Lua54 => LuaVersion::Lua54,
            Self::Luajit => LuaVersion::LuaJIT,
            Self::Luau => LuaVersion::Luau,
        }
    }
}

impl FromStr for Dialect {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, false)
    }
}
