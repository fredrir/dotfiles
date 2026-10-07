use std::path::Path;

use clap::ValueEnum;
use stylua_lib::LuaVersion;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
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
