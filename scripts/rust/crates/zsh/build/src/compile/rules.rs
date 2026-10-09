//! Statements the compiler replaces: sources, helpers it can evaluate now,
//! and the functions oh-my-zsh loads its files through.

use std::path::{Path, PathBuf};

use zshrs_parse::parser::{RedirType, ZshRedir, ZshSimple};

use super::{Compiler, Cx, Mode, Origin, unique, walk};
use crate::expand::{self, Env, Mode as Expand, Var};
use crate::native;
use crate::quote;
use crate::script::{self, Script};

/// Helper definitions the rules below reproduce; any other body is left to run.
const HELPERS: &[(&str, u64)] = &[
    ("add_path", 0x2a20_51ac_049b_befa),
    ("add_plugin_path", 0xc225_d977_0b51_6305),
    ("add_plugins", 0x8b6e_264c_e287_3e54),
    ("add_fpath", 0x6d14_a6de_c586_6662),
    ("has_cmd", 0x1b16_8d4d_4e28_aec0),
    ("cached_eval", 0xca06_b1b9_892c_8a18),
    ("_omz_source", 0xea92_6805_11a2_f97f),
    ("defer", 0xa758_6708_26e4_ee04),
];

pub fn expected(name: &str) -> Option<u64> {
    HELPERS
        .iter()
        .find(|(helper, _)| *helper == name)
        .map(|(_, fingerprint)| *fingerprint)
}

/// Statements worth unrolling a loop for.
pub fn folds(simple: &ZshSimple) -> bool {
    walk::words(simple).first().is_some_and(|name| {
        matches!(
            name.as_str(),
            "source"
                | "."
                | "cached_eval"
                | "_omz_source"
                | "add_path"
                | "add_plugin_path"
                | "add_plugins"
                | "add_fpath"
        )
    })
}

pub fn discards_stderr(redir: &ZshRedir) -> bool {
    redir.fd == 2
        && matches!(redir.rtype, RedirType::Write | RedirType::Writenow)
        && walk::text(&redir.name) == "/dev/null"
}

impl Compiler {
    fn helper_ok(&mut self, name: &str) -> bool {
        let Some(function) = self.state.functions.get(name) else {
            return false;
        };
        if expected(name) == Some(function.fingerprint) {
            return true;
        }
        let note = format!(
            "{name}: unrecognized definition {:#018x}; left to runtime",
            function.fingerprint
        );
        self.note(note);
        false
    }

    fn inline(
        &mut self,
        script: &Script,
        origin: Option<&Path>,
        source: Option<Origin>,
        mode: Mode,
        rewrite: bool,
        cx: &Cx,
    ) -> Option<String> {
        let analysis = cx.analysis || !rewrite;
        let snapshot = self.snapshot();
        let attempt = self.compile_in(script, origin, source, mode, analysis);
        match attempt {
            Ok(text) if !analysis => {
                self.inlined += 1;
                Some(text)
            }
            Ok(_) => None,
            Err(reason) => {
                self.restore(snapshot);
                self.left_to_runtime(origin, &reason);
                let _ = self.compile_in(script, origin, source, mode, true);
                None
            }
        }
    }

    fn compile_in(
        &mut self,
        script: &Script,
        origin: Option<&Path>,
        source: Option<Origin>,
        mode: Mode,
        analysis: bool,
    ) -> Result<String, String> {
        let line = self.line;
        let cx = Cx {
            mode,
            origin,
            analysis,
            source,
        };
        if mode == Mode::Function {
            self.state.push_frame();
        }
        let result = self.compile(script, &cx);
        if mode == Mode::Function {
            self.state.pop_frame();
        }
        self.line = line;
        result
    }

    /// Code generated at the current statement, reported as defined there.
    fn inline_generated(
        &mut self,
        script: &Script,
        mode: Mode,
        rewrite: bool,
        cx: &Cx,
    ) -> Option<String> {
        let location = self.location(cx);
        let source = location.as_deref().map(Origin::Fixed);
        self.inline(script, None, source, mode, rewrite, cx)
    }

    fn inline_file(&mut self, path: &Path, mode: Mode, rewrite: bool, cx: &Cx) -> Option<String> {
        if self.stack.iter().any(|open| open == path) {
            return None;
        }
        let script = match super::read_text(path).and_then(Script::parse) {
            Ok(script) => script,
            Err(error) => {
                self.left_to_runtime(Some(path), &error);
                return None;
            }
        };
        self.stack.push(path.to_path_buf());
        let text = self.inline(
            &script,
            Some(path),
            path.to_str().map(Origin::File),
            mode,
            rewrite,
            cx,
        );
        self.stack.pop();
        text
    }

    fn static_args(&self, args: &[String]) -> Option<Vec<String>> {
        args.iter()
            .map(|arg| expand::words(arg, &self.state, Expand::Args))
            .collect::<Option<Vec<_>>>()
            .map(|values| values.concat())
    }
}

pub fn source(
    compiler: &mut Compiler,
    args: &[String],
    rewrite: bool,
    cx: &Cx,
) -> (Option<String>, bool) {
    let [arg] = args else {
        return (None, false);
    };
    if let Some(inner) = arg
        .strip_prefix("<(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return (generated(compiler, inner, rewrite, cx), false);
    }
    let Some(path) = expand::scalar(arg, &compiler.state).filter(|path| path.starts_with('/'))
    else {
        return (None, false);
    };
    let path = PathBuf::from(path);
    if !path.is_file() {
        return (None, false);
    }
    let wanted = compiler.deferral.file(&path);
    let text = compiler.deferred(wanted, &path, cx, |compiler| {
        compiler.inline_file(&path, Mode::Plain, rewrite, cx)
    });
    (text, false)
}

/// `source <(command)` for a listed command: its output, inlined.
fn generated(compiler: &mut Compiler, inner: &str, rewrite: bool, cx: &Cx) -> Option<String> {
    let argv = substituted(compiler, inner)?;
    let script = output(compiler, &argv)?;
    compiler.inline_generated(&script, Mode::Plain, rewrite, cx)
}

/// `eval` of a command's output: a listed command's output inlined, or
/// native code for an emulated command.
pub fn eval(
    compiler: &mut Compiler,
    simple: &ZshSimple,
    args: &[String],
    rewrite: bool,
    cx: &Cx,
) -> (Option<String>, bool) {
    let [arg] = args else {
        return (None, false);
    };
    let Some((inner, quoted)) = substitution(arg) else {
        return (None, false);
    };
    let Some(argv) = substituted(compiler, inner) else {
        return (None, false);
    };
    if native::emulates(&argv) {
        return (emulated(compiler, simple, rewrite), false);
    }
    if !quoted {
        return (None, false);
    }
    let text = output(compiler, &argv)
        // A `return` in eval'd code leaves the caller, not the code.
        .filter(|script| walk::file_returns(&script.program.lists) == Some(0))
        .and_then(|script| compiler.inline_generated(&script, Mode::Plain, rewrite, cx));
    (text, false)
}

/// The arguments of a substituted command, when they are fixed.
fn substituted(compiler: &Compiler, inner: &str) -> Option<Vec<String>> {
    let program = script::parse(inner).ok()?;
    let [list] = program.lists.as_slice() else {
        return None;
    };
    let simple = walk::sole_simple(list)?;
    if !simple.assigns.is_empty() || !simple.redirs.iter().all(discards_stderr) {
        return None;
    }
    compiler.static_args(&walk::words(simple))
}

/// A listed command's output, parsed.
fn output(compiler: &mut Compiler, argv: &[String]) -> Option<Script> {
    if !compiler.folder.applies(argv, &compiler.state) {
        return None;
    }
    let output = match compiler.folder.run(argv, &compiler.state) {
        Ok(output) => output,
        Err(error) => {
            compiler.note(format!("fold {error}"));
            return None;
        }
    };
    Script::parse(output).ok()
}

/// The command in `$(...)`, `"$(...)"` or backquotes, and whether it is quoted.
fn substitution(arg: &str) -> Option<(&str, bool)> {
    let (arg, quoted) = match arg
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(inner) => (inner, true),
        None => (arg, false),
    };
    if let Some(inner) = arg
        .strip_prefix("$(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return Some((inner, quoted));
    }
    arg.strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .filter(|inner| !inner.contains(['\\', '`', '"']))
        .map(|inner| (inner, quoted))
}

fn emulated(compiler: &mut Compiler, simple: &ZshSimple, rewrite: bool) -> Option<String> {
    compiler.state.forget("PATH");
    compiler.state.forget("MANPATH");
    let original = script::reprint(simple)?;
    let helper = match native::PathHelper::load(&compiler.path_helper_root) {
        Ok(helper) => helper,
        Err(error) => {
            let note = format!("path_helper: {error}; left to runtime");
            if !compiler.skipped.contains(&note) {
                compiler.skipped.push(note);
            }
            return None;
        }
    };
    let text = helper.code(&original);
    script::parse(&text).ok()?;
    compiler.dependencies.extend(helper.sources);
    rewrite.then_some(text)
}

/// The native expansion for `original`, a substitution of `inner`, when the
/// command here prints what its native code makes.
pub fn substitute(
    compiler: &mut Compiler,
    inner: &str,
    original: &str,
) -> Option<(String, &'static str)> {
    let argv = substituted(compiler, inner)?;
    let substitute = native::substitute(&argv)?;
    if !matches!(compiler.state.command(&argv[0]), Some(Some(_))) {
        return None;
    }
    let matches = compiler
        .folder
        .run(&argv, &compiler.state)
        .is_ok_and(|output| (substitute.matches)(&output));
    if !matches {
        let note = format!("{}: unexpected output; left to runtime", argv.join(" "));
        if !compiler.skipped.contains(&note) {
            compiler.skipped.push(note);
        }
        return None;
    }
    Some((substitute.expansion(original), substitute.helper))
}

pub fn cached_eval(
    compiler: &mut Compiler,
    args: &[String],
    rewrite: bool,
    cx: &Cx,
) -> (Option<String>, bool) {
    if !compiler.helper_ok("cached_eval") {
        return (None, false);
    }
    let Some(argv) = compiler.static_args(args).filter(|argv| argv.len() >= 2) else {
        return (None, false);
    };
    let (name, command) = (&argv[0], &argv[1..]);
    match compiler.state.command(&command[0]) {
        Some(Some(_)) => {}
        Some(None) => return (rewrite.then(|| ":\n".to_string()), false),
        None => return (None, false),
    }
    let Ok(output) = compiler.folder.run(command, &compiler.state) else {
        return (rewrite.then(|| ":\n".to_string()), false);
    };
    let Ok(script) = Script::parse(output) else {
        compiler.note(format!(
            "cached_eval {}: output does not parse",
            command.join(" ")
        ));
        return (None, false);
    };
    let origin = match compiler.state.var("DOTFILES_ZSH_CACHE") {
        Var::Scalar(cache) => Some(PathBuf::from(cache).join(format!("{name}.zsh"))),
        _ => None,
    };
    let place = origin.clone().unwrap_or_else(|| PathBuf::from(name));
    let wanted = compiler.deferral.eval(name);
    let text = compiler.deferred(wanted, &place, cx, |compiler| {
        let text = match origin.as_deref() {
            Some(origin) => {
                let source = origin.to_str().map(Origin::File);
                compiler.inline(&script, Some(origin), source, Mode::Function, rewrite, cx)
            }
            None => compiler.inline_generated(&script, Mode::Function, rewrite, cx),
        };
        text.map(|text| function_call(&text, command))
    });
    (text, false)
}

pub fn omz_source(
    compiler: &mut Compiler,
    args: &[String],
    rewrite: bool,
    cx: &Cx,
) -> (Option<String>, bool) {
    if !compiler.helper_ok("_omz_source") || compiler.state.omz_alias_styles {
        return (None, false);
    }
    let [arg] = args else {
        return (None, false);
    };
    let Some(relative) = expand::scalar(arg, &compiler.state) else {
        return (None, false);
    };
    let (Var::Scalar(zsh), custom) = (compiler.state.var("ZSH"), compiler.state.var("ZSH_CUSTOM"))
    else {
        return (None, false);
    };
    let custom = match custom {
        Var::Scalar(custom) => custom,
        Var::Unset => String::new(),
        _ => return (None, false),
    };
    let path = [format!("{custom}/{relative}"), format!("{zsh}/{relative}")]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file());
    let Some(path) = path else {
        return (rewrite.then(|| ":\n".to_string()), false);
    };
    let wanted = compiler.deferral.plugin(&relative);
    let text = compiler.deferred(wanted, &path, cx, |compiler| {
        compiler
            .inline_file(&path, Mode::Function, rewrite, cx)
            .map(|text| function_call(&text, std::slice::from_ref(&relative)))
    });
    (text, false)
}

fn function_call(body: &str, args: &[String]) -> String {
    let args: Vec<String> = args.iter().map(|arg| quote::word(arg)).collect();
    let newline = if body.ends_with('\n') { "" } else { "\n" };
    format!("() {{\n{body}{newline}}} {}\n", args.join(" "))
}

pub fn helper(
    compiler: &mut Compiler,
    name: &str,
    args: &[String],
    rewrite: bool,
) -> (Option<String>, bool) {
    let touched: &[&str] = match name {
        "add_path" => &["path"],
        "add_plugin_path" => &["zsh_plugin_path"],
        "add_plugins" => &["plugins", "zsh_plugin_sources"],
        _ => &[],
    };
    let result = if compiler.helper_ok(name) {
        compiler
            .static_args(args)
            .and_then(|values| evaluate(compiler, name, &values))
    } else {
        None
    };
    match result {
        Some(text) => (rewrite.then_some(text), false),
        None => {
            touched
                .iter()
                .for_each(|variable| compiler.state.forget(variable));
            (None, false)
        }
    }
}

fn evaluate(compiler: &mut Compiler, name: &str, values: &[String]) -> Option<String> {
    let dirs: Vec<String> = values
        .iter()
        .filter(|dir| Path::new(dir).is_dir())
        .cloned()
        .collect();
    match name {
        "add_path" => {
            let paths: Vec<PathBuf> = dirs.iter().map(PathBuf::from).collect();
            compiler.state.prepend_path(&paths);
            Some(if dirs.is_empty() {
                ":\n".to_string()
            } else {
                format!("path=({} \"${{path[@]}}\")\n", words(&dirs))
            })
        }
        "add_plugin_path" => {
            append_unique(compiler, "zsh_plugin_path", &dirs)?;
            Some(array_append("zsh_plugin_path", &dirs))
        }
        "add_fpath" => {
            let reversed: Vec<String> = dirs.iter().rev().cloned().collect();
            Some(if dirs.is_empty() {
                ":\n".to_string()
            } else {
                format!("fpath=({} \"${{fpath[@]}}\")\n", words(&reversed))
            })
        }
        "add_plugins" => {
            let (sources, plugins) = plugins(compiler, values)?;
            append_unique(compiler, "zsh_plugin_sources", &sources)?;
            append_unique(compiler, "plugins", &plugins)?;
            let text =
                array_append("zsh_plugin_sources", &sources) + &array_append("plugins", &plugins);
            Some(text)
        }
        _ => None,
    }
}

/// `add_plugins`: a standalone plugin file when one exists, else an oh-my-zsh plugin.
fn plugins(compiler: &Compiler, specs: &[String]) -> Option<(Vec<String>, Vec<String>)> {
    let search = match compiler.state.var("zsh_plugin_path") {
        Var::Array(dirs) => dirs,
        Var::Unset => Vec::new(),
        _ => return None,
    };
    let Var::Scalar(zsh) = compiler.state.var("ZSH") else {
        return None;
    };
    let mut sources = Vec::new();
    let mut plugins = Vec::new();
    'specs: for spec in specs {
        let name = spec.rsplit(':').next().unwrap_or(spec);
        let command = spec.split(':').next().unwrap_or(spec);
        if compiler.state.command(command)?.is_none() {
            continue;
        }
        for dir in &search {
            for file in [
                format!("{dir}/{name}/{name}.zsh"),
                format!("{dir}/{name}.zsh"),
            ] {
                if std::fs::File::open(&file)
                    .is_ok_and(|file| file.metadata().is_ok_and(|meta| meta.is_file()))
                {
                    sources.push(file);
                    continue 'specs;
                }
            }
        }
        if Path::new(&format!("{zsh}/plugins/{name}")).is_dir() {
            plugins.push(name.to_string());
        }
    }
    Some((sources, plugins))
}

fn append_unique(compiler: &mut Compiler, name: &str, values: &[String]) -> Option<()> {
    let current = match compiler.state.var(name) {
        Var::Array(current) => current,
        Var::Unset => Vec::new(),
        _ => return None,
    };
    let next = unique(current.into_iter().chain(values.iter().cloned()).collect());
    compiler.state.set(name, Var::Array(next));
    Some(())
}

fn array_append(name: &str, values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    format!("{name}+=({})\n", words(values))
}

fn words(values: &[String]) -> String {
    values
        .iter()
        .map(|value| quote::word(value))
        .collect::<Vec<_>>()
        .join(" ")
}
