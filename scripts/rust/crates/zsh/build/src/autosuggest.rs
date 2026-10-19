//! zsh-autosuggestions wraps every widget the first time it starts, with an
//! `eval` per widget. The build starts a shell from the bundle, records what
//! that binding defines, and emits it as code that runs while the widgets and
//! the plugin's settings are what they were then; otherwise the plugin binds them.

use std::path::Path;

use crate::cache::{Cache, Key};
use crate::quote;
use crate::script;
use crate::verify::shell::{self, Startup};

/// The function whose first run is recorded.
pub const BIND: &str = "_zsh_autosuggest_bind_widgets";

/// Stands for the binding code after a compiled zsh-autosuggestions.
pub const PLACEHOLDER: &str = ": __zb_autosuggest\n";

/// `_zsh_build_autosuggest_key`: what the binding reads, as one string.
const KEY: &str = include_str!("../assets/autosuggest.zsh");

const HOOK: &str = include_str!("../assets/capture.zsh");

/// The one parameter the binding sets.
const COUNTS: &str = "_ZSH_AUTOSUGGEST_BIND_COUNTS";

/// What the first binding changed, from a shell whose key was `key`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Binding {
    pub key: String,
    pub functions: Vec<(String, String)>,
    pub widgets: Vec<(String, String)>,
    pub counts: Vec<(String, String)>,
}

impl Binding {
    /// The records `assets/capture.zsh` dumps; an error for a change the
    /// code cannot make.
    pub fn parse(dump: &str) -> Result<Self, String> {
        let mut fields = dump.split('\0');
        let mut binding = Binding::default();
        loop {
            let (Some(table), Some(name), Some(value)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err("record incomplete".to_string());
            };
            let pair = || (name.to_string(), value.to_string());
            match table {
                "end" => return Ok(binding),
                "key" => binding.key = value.to_string(),
                "functions" => binding.functions.push(pair()),
                "widgets" => binding.widgets.push(pair()),
                "counts" => binding.counts.push(pair()),
                "parameters" if name == COUNTS => {}
                "parameters" => return Err(format!("binding sets {name}")),
                "removed" => return Err(format!("binding removes {name} {value}")),
                "unbound" => return Err("binding did not run".to_string()),
                other => return Err(format!("unknown record {other}")),
            }
        }
    }

    /// The bundle's code after zsh-autosuggestions: its first binding makes
    /// these changes while the key holds, then the plugin's own binding is back.
    pub fn code(&self) -> Result<String, String> {
        let mut definitions = String::new();
        for (name, body) in &self.functions {
            if name.is_empty() || !name.bytes().all(quote::is_plain) {
                return Err(format!("function {name}: name not plain"));
            }
            definitions.push_str(&format!("{name}() {{\n{body}\n}}\n"));
        }
        let mut bindings = String::new();
        for (name, value) in &self.widgets {
            let widget = quote::word(name);
            let line = match value.split_once(':') {
                Some(("user", function)) => {
                    format!("zle -N -- {widget} {}\n", quote::word(function))
                }
                Some(("completion", rest)) => {
                    let (builtin, function) = rest
                        .split_once(':')
                        .ok_or_else(|| format!("widget {name}: {value}"))?;
                    format!(
                        "zle -C -- {widget} {} {}\n",
                        quote::word(builtin),
                        quote::word(function)
                    )
                }
                _ => return Err(format!("widget {name}: {value}")),
            };
            bindings.push_str(&line);
        }
        let counts: Vec<String> = self
            .counts
            .iter()
            .map(|(name, count)| format!("{} {}", quote::word(name), quote::word(count)))
            .collect();
        let code = format!(
            "{KEY}_zsh_build_bound() {{\n\
             local REPLY\n\
             _zsh_build_autosuggest_key\n\
             [[ $REPLY == {key} ]] || return 1\n\
             {definitions}{bindings}typeset -gA {COUNTS}=({counts})\n\
             }}\n\
             builtin functions -c {BIND} _zsh_build_bind_widgets\n\
             {BIND}() {{\n\
             builtin functions -c _zsh_build_bind_widgets {BIND}\n\
             builtin unfunction _zsh_build_bind_widgets\n\
             _zsh_build_bound\n\
             local bound=$?\n\
             builtin unfunction _zsh_build_bound _zsh_build_autosuggest_key\n\
             (( bound == 0 )) || {BIND} \"$@\"\n\
             }}\n",
            key = quote::ansi(&self.key),
            counts = counts.join(" "),
        );
        script::parse(&code).map_err(|error| format!("binding code does not parse: {error}"))?;
        Ok(code)
    }
}

/// What a shell started from `bundle` binds; kept between builds by the
/// bundle's text. `record` runs the shell, after `prepare` puts the bundle in place.
pub fn binding(
    cache: &mut Cache,
    zsh: &Path,
    bundle: &Path,
    text: &str,
    ambient: &[String],
    record: bool,
    prepare: impl FnOnce() -> Result<(), String>,
) -> Result<Binding, String> {
    let hook = format!("{KEY}{HOOK}");
    let identity = [
        "autosuggest-binding".to_string(),
        format!("{:016x}", script::hash(text)),
        format!("{:016x}", script::hash(&hook)),
    ];
    let dump = cache.get_or_run(Key::new(&identity, zsh, None), || {
        if !record {
            return Err("not recorded".to_string());
        }
        prepare()?;
        let scratch = tempfile::Builder::new()
            .prefix("zsh-build-autosuggest.")
            .tempdir()
            .map_err(|error| format!("scratch directory: {error}"))?;
        let env = shell::environment(ambient);
        let dump = shell::run(
            zsh,
            scratch.path(),
            &env,
            &Startup::Bundle(&[], Some(bundle)),
            &hook,
        )?;
        Ok(String::from_utf8(dump).map_err(|_| "record not UTF-8".to_string()))
    })?;
    Binding::parse(&dump)
}

#[cfg(test)]
#[path = "../tests/unit/autosuggest_tests.rs"]
mod tests;
