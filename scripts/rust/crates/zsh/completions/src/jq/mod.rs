pub mod builtins;
mod filter;
mod flags;
mod lexer;
mod modules;
mod probe;
mod shell;

use std::fs;
use std::path::{Path, PathBuf};

use crate::context::Context;
use crate::line::Line;
use crate::reply::{Group, Item, Reply, Tone};
use builtins::Function;
use filter::{Analysis, Program, Want};
use flags::{Flag, Value};
use probe::{Key, Run};
use shell::Input;

const JSON_FILES: &str = "*.(json|jsonl|ndjson|geojson|jsonld|har|ipynb)(-.)";
const JQ_FILES: &str = "*.jq(-.)";
const INPUT_LIMIT: u64 = 64 << 20;

pub fn complete(ctx: &Context, line: &Line) -> Reply {
    let command = line.words.first().map_or("jq", String::as_str);
    let jq = if command.contains('/') {
        Some(ctx.expand(command)).filter(|path| crate::context::is_executable(path))
    } else {
        ctx.which("jq")
    };
    let mut reply = Reply::new();
    Completer {
        ctx,
        line,
        jq,
        scan: Scan::new(line),
    }
    .run(&mut reply);
    reply
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cursor {
    Flag,
    Value(&'static Flag, usize),
    Filter,
    File,
    Argument(&'static str),
    // Right after a flag this module does not know, so its value is unknown too.
    Unknown,
}

// The command line as jq would read it, and the role of the word under the cursor.
#[derive(Debug)]
struct Scan {
    cursor: Cursor,
    filter: Option<String>,
    used: Vec<&'static Flag>,
    probe: Vec<(&'static Flag, Vec<String>)>,
    library: Vec<String>,
    named: Vec<(&'static Flag, String, String)>,
    files: Vec<String>,
    mode: Option<&'static str>,
    positionals: Vec<String>,
    null_input: bool,
    from_file: bool,
}

impl Scan {
    fn new(line: &Line) -> Scan {
        let mut scan = Scan {
            cursor: Cursor::File,
            filter: None,
            used: Vec::new(),
            probe: Vec::new(),
            library: Vec::new(),
            named: Vec::new(),
            files: Vec::new(),
            mode: None,
            positionals: Vec::new(),
            null_input: false,
            from_file: false,
        };
        let mut pending: Option<(&'static Flag, Vec<String>)> = None;
        let mut options = true;
        let mut filter_seen = false;
        let mut unknown_at = None;
        for (index, raw) in line.words.iter().enumerate().skip(1) {
            let at_cursor = index == line.current;
            let word = if at_cursor {
                line.prefix.clone()
            } else {
                shell::unquote(raw)
            };
            if let Some((flag, mut values)) = pending.take() {
                if at_cursor {
                    scan.cursor = Cursor::Value(flag, values.len());
                }
                values.push(word);
                if values.len() == flag.values.len() {
                    scan.set(flag, values);
                } else {
                    pending = Some((flag, values));
                }
                continue;
            }
            let quoted = raw.starts_with(['\'', '"']);
            if at_cursor {
                scan.cursor = if options && word.starts_with('-') && !quoted {
                    Cursor::Flag
                } else if unknown_at == Some(index - 1) {
                    Cursor::Unknown
                } else if !filter_seen && !scan.from_file {
                    filter_seen = true;
                    Cursor::Filter
                } else if let Some(mode) = scan.mode {
                    Cursor::Argument(mode)
                } else {
                    Cursor::File
                };
                continue;
            }
            if options && word == "--" {
                options = false;
            } else if options
                && !quoted
                && let Some(name) = word.strip_prefix("--")
            {
                match flags::long(name) {
                    Some(flag) if flag.values.is_empty() => scan.set(flag, Vec::new()),
                    Some(flag) => pending = Some((flag, Vec::new())),
                    None => unknown_at = Some(index),
                }
            } else if options && word.starts_with('-') && word.len() > 1 && !quoted {
                // Short flags combine, as in `-nr`; a value may follow its flag directly.
                for (at, short) in word.char_indices().skip(1) {
                    let Some(flag) = flags::short(short) else {
                        unknown_at = Some(index);
                        break;
                    };
                    if flag.values.is_empty() {
                        scan.set(flag, Vec::new());
                        continue;
                    }
                    let rest = &word[at + short.len_utf8()..];
                    if rest.is_empty() {
                        pending = Some((flag, Vec::new()));
                    } else {
                        scan.set(flag, vec![rest.to_string()]);
                    }
                    break;
                }
            } else if !filter_seen && !scan.from_file {
                filter_seen = true;
                scan.filter = Some(word);
            } else if scan.mode.is_some() {
                scan.positionals.push(word);
            } else {
                scan.files.push(word);
            }
        }
        scan
    }

    fn set(&mut self, flag: &'static Flag, values: Vec<String>) {
        self.used.push(flag);
        match (flag.long, values.as_slice()) {
            ("null-input", _) => self.null_input = true,
            ("from-file", _) => self.from_file = true,
            ("args" | "jsonargs", _) => self.mode = Some(flag.long),
            ("library-path", [dir]) => self.library.push(dir.clone()),
            ("arg" | "argjson" | "slurpfile" | "rawfile", [name, value]) => {
                self.named.push((flag, name.clone(), value.clone()));
            }
            _ => {}
        }
        if flag.probe {
            self.probe.push((flag, values));
        }
    }
}

struct Completer<'a> {
    ctx: &'a Context,
    line: &'a Line,
    jq: Option<PathBuf>,
    scan: Scan,
}

impl Completer<'_> {
    fn run(&self, reply: &mut Reply) {
        match self.scan.cursor {
            Cursor::Flag => self.flags(reply),
            Cursor::Value(flag, slot) => self.value(flag, slot, reply),
            Cursor::Filter => self.filter(reply),
            Cursor::File => reply.files_matching(JSON_FILES),
            Cursor::Argument("args") => reply.message("string argument"),
            Cursor::Argument(_) => reply.message("JSON argument"),
            Cursor::Unknown => reply.delegate(),
        }
    }

    fn flags(&self, reply: &mut Reply) {
        let alone = self.line.words.len() <= 2;
        let used = &self.scan.used;
        let items = flags::FLAGS
            .iter()
            .filter(|flag| flag.repeatable || !used.contains(flag))
            .filter(|flag| {
                !used.iter().any(|known| {
                    known.excludes.contains(&flag.long) || flag.excludes.contains(&known.long)
                })
            })
            .filter(|flag| alone || !flag.alone)
            .flat_map(|flag| flag.names().map(|name| Item::new(name, flag.description)));
        reply.group(
            Group::new("options", "option")
                .tone(Tone::Info)
                .items(items),
        );
    }

    fn value(&self, flag: &Flag, slot: usize, reply: &mut Reply) {
        match flag.values.get(slot) {
            Some(Value::Indent) => reply.group(
                Group::new("values", "spaces")
                    .unsorted()
                    .items((0..=7).map(|spaces: u8| Item::bare(spaces.to_string()))),
            ),
            Some(Value::Program) => reply.files_matching(JQ_FILES),
            Some(Value::Directory) => reply.directories(),
            Some(Value::Name) => {
                let named: Vec<&str> = self
                    .scan
                    .named
                    .iter()
                    .map(|(_, name, _)| name.as_str())
                    .collect();
                let free = self
                    .scan
                    .filter
                    .as_deref()
                    .map(filter::free_variables)
                    .unwrap_or_default();
                reply.group(
                    Group::new("variables", "variable")
                        .tone(Tone::Success)
                        .items(
                            free.into_iter()
                                .filter(|name| !named.contains(&name.as_str()))
                                .map(Item::bare),
                        ),
                );
                reply.message("variable name");
            }
            Some(Value::Text) => reply.message("string"),
            Some(Value::Json) => reply.message("JSON value"),
            Some(Value::File) => reply.files(),
            Some(Value::JsonFile) => reply.files_matching(JSON_FILES),
            None => {}
        }
    }

    fn filter(&self, reply: &mut Reply) {
        let raw = &self.line.words[self.line.current];
        let prefix = &self.line.prefix;
        let (text, offsets) = unescape(prefix, raw);
        let analysis = filter::analyze(&text);
        if analysis.want != Want::Nothing {
            reply.skip(&prefix[..offsets[analysis.skip]]);
        }
        match &analysis.want {
            Want::Nothing => {}
            Want::Field | Want::QuotedField | Want::Index | Want::ObjectKey => {
                self.keys(&analysis, reply);
            }
            Want::Term => {
                self.keys(&analysis, reply);
                self.functions(&analysis, reply);
                self.keywords(&analysis, reply);
            }
            Want::Operator => self.keywords(&analysis, reply),
            Want::Variable => self.variables(&analysis, reply),
            Want::Format => reply.group(
                Group::new("formats", "format")
                    .tone(Tone::Info)
                    .unsorted()
                    .suffix("", false)
                    .items(builtins::FORMATS.map(|(name, about)| Item::new(name, about))),
            ),
            Want::Module { data } => self.modules(*data, reply),
            Want::ModuleFunction { module } => self.module_functions(&analysis, module, reply),
        }
    }

    fn keys(&self, analysis: &Analysis, reply: &mut Reply) {
        let environment =
            analysis.want == Want::Field && matches!(analysis.base.as_str(), "$ENV" | "env");
        let keys = if environment {
            self.environment()
        } else {
            self.probe(analysis, reply)
        };
        let item = |key: &Key, value: String| {
            Item::bare(value)
                .detail(Tone::Info, key.types.join("|"))
                .detail(Tone::Muted, &key.preview)
        };
        // The escaped key without its quotes, so the typed word matches it.
        let inner = |key: &Key| {
            let quoted = json(&key.name);
            item(key, quoted[1..quoted.len() - 1].to_string())
        };
        let group = || Group::new("keys", "key").unsorted().suffix("", false);
        let (plain, quoted): (Vec<&Key>, Vec<&Key>) =
            keys.iter().partition(|key| is_identifier(&key.name));
        match analysis.want {
            Want::Field | Want::ObjectKey => {
                reply.group(group().items(plain.iter().map(|key| item(key, key.name.clone()))));
                reply.group(
                    group()
                        .insert_prefix("\"")
                        .suffix("\"", false)
                        .items(quoted.iter().map(|key| inner(key))),
                );
            }
            Want::QuotedField | Want::Index => {
                let suffix = if analysis.want == Want::Index {
                    "\"]"
                } else {
                    "\""
                };
                reply.group(group().suffix(suffix, false).items(keys.iter().map(inner)));
            }
            Want::Term => reply.group(group().items(keys.iter().map(|key| {
                let name = if is_identifier(&key.name) {
                    key.name.clone()
                } else {
                    json(&key.name)
                };
                item(key, format!(".{name}"))
            }))),
            _ => {}
        }
    }

    // Values stay hidden, since the environment holds tokens and passwords.
    fn environment(&self) -> Vec<Key> {
        self.ctx
            .vars()
            .map(|(name, _)| Key {
                name: name.to_string(),
                types: vec!["string".into()],
                preview: String::new(),
            })
            .collect()
    }

    fn probe(&self, analysis: &Analysis, reply: &mut Reply) -> Vec<Key> {
        let (Some(jq), Some(program)) = (&self.jq, &analysis.program) else {
            return Vec::new();
        };
        let Some(run) = self.invocation(reply) else {
            return Vec::new();
        };
        let cwd = &self.ctx.cwd;
        let probed = probe::keys(jq, cwd, &run, program, &analysis.partial);
        if probed.compiled || analysis.base.is_empty() {
            return probed.keys;
        }
        // Around a construct this module cannot close, the path being typed still has keys.
        let path = Program {
            before: format!("{} | ", analysis.base),
            after: String::new(),
        };
        probe::keys(jq, cwd, &run, &path, &analysis.partial).keys
    }

    // jq fed as the command line would feed it, or None when it would read nothing.
    fn invocation(&self, reply: &mut Reply) -> Option<Run> {
        let mut run = Run::default();
        for (flag, values) in &self.scan.probe {
            run.flags.push(format!("--{}", flag.long));
            for (value, kind) in values.iter().zip(flag.values) {
                let path = matches!(kind, Value::Directory | Value::File | Value::JsonFile);
                run.flags.push(if path {
                    self.path(value)
                } else {
                    value.clone()
                });
            }
        }
        if let Some(mode) = self.scan.mode {
            run.trailing.push(format!("--{mode}"));
            run.trailing.extend(self.scan.positionals.iter().cloned());
        }
        let mut inputs: Vec<Input> = self.scan.files.iter().cloned().map(Input::File).collect();
        if inputs.is_empty() {
            inputs = self.stdin();
        }
        let inputs: Vec<Input> = inputs
            .into_iter()
            .filter_map(|input| match input {
                Input::File(file) => {
                    let path = self.path(&file);
                    Path::new(&path).is_file().then_some(Input::File(path))
                }
                text => Some(text),
            })
            .collect();
        if inputs.is_empty() && !self.scan.null_input {
            return None;
        }
        let size: u64 = inputs
            .iter()
            .map(|input| match input {
                Input::File(file) => fs::metadata(file).map_or(0, |meta| meta.len()),
                Input::Text(text) => text.len() as u64,
            })
            .sum();
        if size > INPUT_LIMIT {
            reply.message("input too large");
            return None;
        }
        if inputs.iter().any(|input| matches!(input, Input::Text(_))) {
            let mut bytes = Vec::new();
            for input in inputs {
                match input {
                    Input::File(file) => bytes.extend(fs::read(file).ok()?),
                    Input::Text(text) => bytes.extend(text.into_bytes()),
                }
            }
            run.stdin = Some(bytes);
        } else {
            run.files = inputs
                .into_iter()
                .filter_map(|input| match input {
                    Input::File(file) => Some(file),
                    Input::Text(_) => None,
                })
                .collect();
        }
        Some(run)
    }

    // What is piped or redirected into jq, read from the line around the current word.
    fn stdin(&self) -> Vec<Input> {
        let Some((left, right)) = &self.line.buffer else {
            return Vec::new();
        };
        let word = shell::unquote(&self.line.words[self.line.current]);
        let (before, after) = around(left, right, &word);
        shell::stdin(before, after)
    }

    fn path(&self, value: &str) -> String {
        self.ctx.expand(value).to_string_lossy().to_string()
    }

    fn functions(&self, analysis: &Analysis, reply: &mut Reply) {
        let mut defined: Vec<Function> = Vec::new();
        let mut describe = |functions: &[(String, usize)], about: &str| {
            let named = functions
                .iter()
                .map(|(name, arity)| (name.as_str(), *arity));
            defined.extend(builtins::functions(named, |_| about.to_string()));
        };
        describe(&analysis.functions, "defined in the filter");
        let dirs = self.search_path();
        for import in &analysis.imports {
            if import.alias.is_none()
                && let Some(path) = modules::resolve(&dirs, &import.path, false)
            {
                describe(&modules::functions(&path), &import.path);
            }
        }
        let home = self.ctx.home.join(".jq");
        if home.is_file() {
            describe(&modules::functions(&home), "~/.jq");
        }
        let builtins = self
            .jq
            .clone()
            .map(|jq| builtins::load(self.ctx, jq))
            .unwrap_or_default();
        let all = defined.iter().chain(&builtins);
        offer_functions(all.map(|function| (function.name.clone(), function)), reply);
        let aliases = analysis
            .imports
            .iter()
            .filter(|import| !import.data)
            .filter_map(|import| {
                Some(Item::new(
                    format!("{}::", import.alias.as_ref()?),
                    &import.path,
                ))
            });
        reply.group(
            Group::new("modules", "module")
                .suffix("", false)
                .items(aliases),
        );
    }

    fn keywords(&self, analysis: &Analysis, reply: &mut Reply) {
        let items = analysis.keywords.iter().map(|keyword| {
            let about = builtins::KEYWORDS
                .iter()
                .find(|(known, _)| known == keyword)
                .map_or("", |(_, about)| about);
            Item::new(*keyword, about)
        });
        reply.group(
            Group::new("keywords", "keyword")
                .tone(Tone::Warning)
                .unsorted()
                .suffix(" ", true)
                .items(items),
        );
    }

    fn variables(&self, analysis: &Analysis, reply: &mut Reply) {
        let mut items: Vec<Item> = analysis
            .variables
            .iter()
            .map(|name| {
                Item::bare(name)
                    .detail(Tone::Info, "")
                    .detail(Tone::Muted, "bound in the filter")
            })
            .collect();
        for (flag, name, value) in &self.scan.named {
            items.push(
                Item::bare(format!("${name}"))
                    .detail(Tone::Info, format!("--{}", flag.long))
                    .detail(Tone::Muted, value),
            );
        }
        for import in analysis.imports.iter().filter(|import| import.data) {
            if let Some(alias) = &import.alias {
                items.push(
                    Item::bare(format!("${alias}::{alias}"))
                        .detail(Tone::Info, "import")
                        .detail(Tone::Muted, &import.path),
                );
            }
        }
        for (name, about) in builtins::VARIABLES {
            items.push(
                Item::bare(name)
                    .detail(Tone::Info, "")
                    .detail(Tone::Muted, about),
            );
        }
        reply.group(
            Group::new("variables", "variable")
                .tone(Tone::Success)
                .unsorted()
                .suffix("", false)
                .items(items),
        );
    }

    fn modules(&self, data: bool, reply: &mut Reply) {
        let items = modules::list(&self.search_path())
            .into_iter()
            .filter(|module| data || !module.data)
            .map(|module| Item::new(module.name, if module.data { "data" } else { "module" }));
        reply.group(
            Group::new("modules", "module")
                .suffix("\"", false)
                .items(items),
        );
    }

    fn module_functions(&self, analysis: &Analysis, module: &str, reply: &mut Reply) {
        let import = analysis
            .imports
            .iter()
            .find(|import| !import.data && import.alias.as_deref() == Some(module));
        let Some(import) = import else { return };
        let Some(path) = modules::resolve(&self.search_path(), &import.path, false) else {
            return;
        };
        let defined = modules::functions(&path);
        let functions = builtins::functions(
            defined.iter().map(|(name, arity)| (name.as_str(), *arity)),
            |_| import.path.clone(),
        );
        let qualified = functions
            .iter()
            .map(|function| (format!("{module}::{}", function.name), function));
        offer_functions(qualified, reply);
    }

    fn search_path(&self) -> Vec<PathBuf> {
        modules::search_path(self.ctx, self.jq.as_deref(), &self.scan.library)
    }
}

// Functions that need arguments open their parentheses.
fn offer_functions<'a>(functions: impl Iterator<Item = (String, &'a Function)>, reply: &mut Reply) {
    let mut bare = Vec::new();
    let mut called = Vec::new();
    for (value, function) in functions {
        let arities: Vec<String> = function
            .arities
            .iter()
            .map(|arity| format!("/{arity}"))
            .collect();
        let item = Item::bare(value)
            .detail(Tone::Info, arities.join(" "))
            .detail(Tone::Muted, &function.description);
        if function.arities.contains(&0) {
            bare.push(item);
        } else {
            called.push(item);
        }
    }
    reply.group(
        Group::new("functions", "function")
            .suffix("", false)
            .items(bare),
    );
    reply.group(
        Group::new("functions", "function")
            .suffix("(", true)
            .items(called),
    );
}

// The filter as jq reads it, and where each of its bytes starts in the shell word.
fn unescape(prefix: &str, raw: &str) -> (String, Vec<usize>) {
    let single = raw.starts_with('\'') || raw.starts_with("$'");
    let double = raw.starts_with('"');
    let mut text = String::with_capacity(prefix.len());
    let mut offsets = Vec::with_capacity(prefix.len() + 1);
    let mut chars = prefix.char_indices().peekable();
    while let Some((at, mut c)) = chars.next() {
        let escaped = c == '\\'
            && !single
            && chars
                .peek()
                .is_some_and(|(_, next)| !double || matches!(next, '$' | '`' | '"' | '\\'));
        if escaped && let Some((_, next)) = chars.next() {
            c = next;
        }
        text.push(c);
        offsets.extend(std::iter::repeat_n(at, c.len_utf8()));
    }
    offsets.push(prefix.len());
    (text, offsets)
}

// The line before and after the current word, which zsh passes with that word's quotes removed.
fn around<'a>(left: &'a str, right: &'a str, word: &str) -> (&'a str, &'a str) {
    let split = (0..=word.len())
        .rev()
        .filter(|at| word.is_char_boundary(*at))
        .find(|at| left.ends_with(&word[..*at]) && right.starts_with(&word[*at..]));
    match split {
        Some(at) => (&left[..left.len() - at], &right[word.len() - at..]),
        None => (left, right),
    }
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn json(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

#[cfg(test)]
#[path = "../../tests/unit/jq/mod_tests.rs"]
mod tests;
