use crate::{Binding, Package, compact, humanize};
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser, Tree};

#[derive(Clone, Debug)]
enum Value {
    String(String),
    Number(i64),
    Bool(bool),
    Nil,
    Table(Vec<(String, Value)>),
    Expression(String),
}

impl Value {
    fn text(&self) -> String {
        match self {
            Self::String(s) | Self::Expression(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Nil => "nil".into(),
            Self::Table(entries) => format!(
                "{{ {} }}",
                entries
                    .iter()
                    .map(|(key, v)| format!("{key} = {}", v.code()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    fn code(&self) -> String {
        match self {
            Self::String(s) => format!("{s:?}"),
            _ => self.text(),
        }
    }
    fn truth(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Nil => Some(false),
            Self::Expression(_) => None,
            _ => Some(true),
        }
    }
    fn get(&self, key: &str) -> Option<Self> {
        if let Self::Table(entries) = self {
            entries
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        } else {
            None
        }
    }
    fn alternatives(&self) -> Vec<String> {
        if let Self::Table(entries) = self {
            entries.iter().map(|(_, v)| v.text()).collect()
        } else {
            vec![self.text()]
        }
    }
}

type Env = BTreeMap<String, Value>;

fn tree(body: &str) -> Result<Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_lua::LANGUAGE.into())
        .map_err(|e| e.to_string())?;
    let tree = parser.parse(body, None).ok_or("Lua parse failed")?;
    if tree.root_node().has_error() {
        fn error(node: Node<'_>) -> Node<'_> {
            for child in children(node) {
                if child.has_error() || child.is_missing() {
                    return error(child);
                }
            }
            node
        }
        let node = error(tree.root_node());
        return Err(format!(
            "{}: invalid Lua near {}",
            node.start_position().row + 1,
            compact(raw(node, body))
        ));
    }
    Ok(tree)
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect()
}
fn field<'a>(node: Node<'a>, name: &str) -> Node<'a> {
    node.child_by_field_name(name).expect("Lua grammar field")
}
fn raw<'a>(node: Node<'_>, body: &'a str) -> &'a str {
    &body[node.byte_range()]
}

fn unquote(s: &str) -> String {
    if let Some(after_bracket) = s.strip_prefix('[') {
        if let Some(end) = after_bracket.find('[') {
            let width = end + 2;
            return s[width..s.len() - width]
                .strip_prefix('\n')
                .unwrap_or(&s[width..s.len() - width])
                .to_string();
        }
        return s.to_string();
    }
    let mut chars = s[1..s.len() - 1].chars().peekable();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            value.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => value.push('\n'),
            Some('t') => value.push('\t'),
            Some('r') => value.push('\r'),
            Some(c @ ('\\' | '\'' | '"')) => value.push(c),
            Some('x') => {
                value.push_str("\\x");
                for _ in 0..2 {
                    if let Some(c) = chars.next() {
                        value.push(c);
                    }
                }
            }
            Some(c) => {
                value.push('\\');
                value.push(c);
            }
            None => {}
        }
    }
    value
}

fn eval(node: Node<'_>, body: &str, env: &Env) -> Value {
    let text = raw(node, body);
    if let Some(value) = env.get(text) {
        return value.clone();
    }
    match node.kind() {
        "string" => Value::String(unquote(text)),
        "number" => text
            .parse()
            .map(Value::Number)
            .unwrap_or_else(|_| Value::Expression(text.into())),
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "nil" => Value::Nil,
        "parenthesized_expression" => eval(children(node)[0], body, env),
        "table_constructor" => {
            let mut entries = Vec::new();
            let mut index = 1;
            for child in children(node).into_iter().filter(|n| n.kind() == "field") {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| {
                        if n.kind() == "identifier" {
                            raw(n, body).to_string()
                        } else {
                            eval(n, body, env).text()
                        }
                    })
                    .unwrap_or_else(|| {
                        let name = index.to_string();
                        index += 1;
                        name
                    });
                entries.push((name, eval(field(child, "value"), body, env)));
            }
            Value::Table(entries)
        }
        "dot_index_expression" | "bracket_index_expression" => {
            let table = eval(field(node, "table"), body, env);
            let key = if node.kind() == "dot_index_expression" {
                raw(field(node, "field"), body).to_string()
            } else {
                eval(field(node, "field"), body, env).text()
            };
            table.get(&key).unwrap_or_else(|| {
                Value::Expression(if node.kind() == "dot_index_expression" {
                    format!("{}.{key}", table.text())
                } else {
                    format!(
                        "{}[{}]",
                        table.text(),
                        eval(field(node, "field"), body, env).code()
                    )
                })
            })
        }
        "binary_expression" => {
            let left = eval(field(node, "left"), body, env);
            let right = eval(field(node, "right"), body, env);
            let op = raw(field(node, "operator"), body);
            match (op, &left, &right) {
                ("and", _, _) if left.truth().is_some() => {
                    if left.truth() == Some(true) {
                        right
                    } else {
                        left
                    }
                }
                ("or", _, _) if left.truth().is_some() => {
                    if left.truth() == Some(true) {
                        left
                    } else {
                        right
                    }
                }
                (
                    "..",
                    Value::String(_) | Value::Number(_),
                    Value::String(_) | Value::Number(_),
                ) => Value::String(left.text() + &right.text()),
                ("+", Value::Number(a), Value::Number(b)) => a
                    .checked_add(*b)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::Expression(compact(text))),
                ("-", Value::Number(a), Value::Number(b)) => a
                    .checked_sub(*b)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::Expression(compact(text))),
                _ => Value::Expression(format!("{} {op} {}", left.code(), right.code())),
            }
        }
        "unary_expression" => {
            let operand = eval(field(node, "operand"), body, env);
            match (raw(field(node, "operator"), body), &operand) {
                ("not", _) if operand.truth().is_some() => Value::Bool(!operand.truth().unwrap()),
                ("-", Value::Number(n)) => n
                    .checked_neg()
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::Expression(compact(text))),
                _ => Value::Expression(compact(text)),
            }
        }
        "function_call" => {
            let name = raw(field(node, "name"), body);
            let args = children(field(node, "arguments"));
            if name == "require"
                && args.len() == 1
                && let Some(value) =
                    env.get(&format!("require:{}", eval(args[0], body, env).text()))
            {
                return value.clone();
            }
            if name == "tostring" && args.len() == 1 {
                let value = eval(args[0], body, env);
                if matches!(value, Value::Number(_) | Value::String(_)) {
                    return Value::String(value.text());
                }
            }
            Value::Expression(format!(
                "{}({})",
                eval(field(node, "name"), body, env).text(),
                args.iter()
                    .map(|n| eval(*n, body, env).code())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        "function_definition" => {
            Value::Expression(format!("callback @L{}", node.start_position().row + 1))
        }
        _ => Value::Expression(compact(text)),
    }
}

fn context(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ")
}

struct Scan<'a> {
    source: &'a str,
    body: &'a str,
    package: &'a str,
    rows: &'a mut Package,
}

impl Scan<'_> {
    fn emit(
        &mut self,
        node: Node<'_>,
        key: Value,
        action: Value,
        description: String,
        scope: &[String],
    ) {
        let mut scope = scope.to_vec();
        if matches!(key, Value::Expression(_)) {
            scope.push("unresolved key expression".into());
        }
        let action = action.text();
        self.rows.bindings.push(Binding {
            context: context(&scope),
            key: key.text(),
            action: action.clone(),
            description: if description.is_empty() {
                describe(&action)
            } else {
                description
            },
            source: self.source.into(),
            line: node.start_position().row + 1,
        });
    }

    fn visit(&mut self, node: Node<'_>, env: &mut Env, scope: &[String]) -> Result<(), String> {
        match node.kind() {
            "assignment_statement" => {
                let nodes = children(node);
                let names = nodes
                    .iter()
                    .find(|n| n.kind() == "variable_list")
                    .map(|n| children(*n))
                    .unwrap_or_default();
                let values = nodes
                    .iter()
                    .find(|n| n.kind() == "expression_list")
                    .map(|n| children(*n))
                    .unwrap_or_default();
                let evaluated: Vec<_> = values.iter().map(|n| eval(*n, self.body, env)).collect();
                for (name, value) in names.iter().zip(values.iter()) {
                    let name = raw(*name, self.body);
                    if self.package == "nvim"
                        && (name.ends_with("mapleader")
                            || name.ends_with("maplocalleader")
                            || name == "M.blink")
                    {
                        self.rows.settings.push(Binding {
                            key: name.into(),
                            action: eval(*value, self.body, env).text(),
                            context: context(scope),
                            source: self.source.into(),
                            line: name_line(*value),
                            ..Binding::default()
                        });
                    }
                    if self.package == "nvim" && name == "M.neo_tree_window" {
                        let mut local = scope.to_vec();
                        local.push("Neo-tree window".into());
                        for item in children(*value).into_iter().filter(|n| n.kind() == "field") {
                            if let Some(key) = item.child_by_field_name("name") {
                                self.emit(
                                    item,
                                    eval(key, self.body, env),
                                    eval(field(item, "value"), self.body, env),
                                    String::new(),
                                    &local,
                                );
                            }
                        }
                    } else {
                        self.visit(*value, env, scope)?;
                    }
                }
                for (name, value) in names.iter().zip(evaluated) {
                    let name = raw(*name, self.body);
                    // Runtime actions remain named references instead of expanding callback bodies.
                    if !matches!(value, Value::Expression(_))
                        || matches!(
                            value.text().as_str(),
                            "vim.keymap.set" | "vim.api.nvim_set_keymap"
                        )
                    {
                        env.insert(name.into(), value);
                    } else {
                        env.remove(name);
                    }
                }
                return Ok(());
            }
            "if_statement" => return self.branch(node, env, scope),
            "for_statement" => return self.loop_body(node, env, scope),
            "function_declaration" | "function_definition" => {
                let mut local = scope.to_vec();
                if let Some(name) = node.child_by_field_name("name") {
                    local.push(raw(name, self.body).to_string());
                }
                let mut local_env = env.clone();
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    for p in children(parameters) {
                        local_env.remove(raw(p, self.body));
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit(body, &mut local_env, &local)?;
                }
                return Ok(());
            }
            "function_call" if self.package == "nvim" => {
                let name = raw(field(node, "name"), self.body);
                if matches!(
                    name,
                    "map"
                        | "buffer_map"
                        | "picker_map"
                        | "vim.keymap.set"
                        | "vim.api.nvim_set_keymap"
                        | "vim.api.nvim_buf_set_keymap"
                ) || env.get(name).is_some_and(|v| {
                    matches!(
                        v.text().as_str(),
                        "vim.keymap.set" | "vim.api.nvim_set_keymap"
                    )
                }) {
                    let args = children(field(node, "arguments"));
                    let offset = usize::from(name == "vim.api.nvim_buf_set_keymap");
                    if args.len() < offset + 3 {
                        return Err(format!("{}: incomplete {name}", name_line(node)));
                    }
                    let key = eval(args[offset + 1], self.body, env);
                    // Forwarding inside mapping helpers is represented by their call sites.
                    if matches!(&key, Value::Expression(s) if s == "lhs" || s == "key")
                        && scope.iter().any(|s| s == "buffer_map")
                    {
                        return Ok(());
                    }
                    let modes = eval(args[offset], self.body, env).alternatives();
                    let mut local = scope.to_vec();
                    local.push(format!("mode={}", modes.join(",")));
                    if name == "buffer_map" || offset == 1 {
                        local.push("buffer-local".into());
                    }
                    if name == "picker_map" {
                        local.push("Telescope picker".into());
                    }
                    let opts = args.get(offset + 3).map(|n| eval(*n, self.body, env));
                    let description = match &opts {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.get("desc").map(|v| v.text()).unwrap_or_default(),
                        None => String::new(),
                    };
                    if let Some(opts) = opts {
                        for option in ["buffer", "expr", "remap", "nowait"] {
                            if let Some(value) = opts.get(option) {
                                local.push(format!("{option}={}", value.text()));
                            }
                        }
                    }
                    self.emit(
                        node,
                        key,
                        eval(args[offset + 2], self.body, env),
                        description,
                        &local,
                    );
                    return Ok(());
                }
            }
            "table_constructor" if self.package == "wezterm" => {
                let value = eval(node, self.body, env);
                if let Some(action) = value.get("action")
                    && let Some(key) = value.get("key").or_else(|| value.get("event"))
                {
                    if matches!(&key, Value::Expression(s) if s == "binding.key")
                        && action.text() == "binding.action"
                    {
                        return Ok(());
                    }
                    let modifiers = value.get("mods").unwrap_or(Value::String("NONE".into()));
                    let mods = modifiers.alternatives();
                    let description = value
                        .get("desc")
                        .or_else(|| value.get("description"))
                        .map(|v| v.text())
                        .unwrap_or_default();
                    let mut local = scope.to_vec();
                    if matches!(modifiers, Value::Expression(_))
                        || matches!(&modifiers, Value::Table(entries) if entries.iter().any(|(_, v)| matches!(v, Value::Expression(_))))
                    {
                        local.push("unresolved modifier expression".into());
                    }
                    if value.get("event").is_some() {
                        local.push("mouse".into());
                    }
                    for modifier in mods {
                        let key = match &key {
                            Value::Expression(s) => Value::Expression(format!("{modifier}+{s}")),
                            _ => Value::String(if modifier == "NONE" || modifier.is_empty() {
                                key.text()
                            } else {
                                format!("{}+{}", modifier.replace('|', "+"), key.text())
                            }),
                        };
                        self.emit(node, key, action.clone(), description.clone(), &local);
                    }
                    return Ok(());
                }
                for setting in [
                    "disable_default_key_bindings",
                    "disable_default_mouse_bindings",
                    "leader",
                ] {
                    if let Some(setting_value) = value.get(setting) {
                        self.rows.settings.push(Binding {
                            key: setting.into(),
                            action: setting_value.text(),
                            context: context(scope),
                            source: self.source.into(),
                            line: name_line(node),
                            ..Binding::default()
                        });
                    }
                }
            }
            _ => {}
        }
        for child in children(node) {
            self.visit(child, env, scope)?;
        }
        Ok(())
    }

    fn branch(&mut self, node: Node<'_>, env: &mut Env, scope: &[String]) -> Result<(), String> {
        let condition = field(node, "condition");
        let known = eval(condition, self.body, env).truth();
        if known != Some(false)
            && let Some(body) = node.child_by_field_name("consequence")
        {
            if known == Some(true) {
                self.visit(body, env, scope)?;
            } else {
                let mut local = scope.to_vec();
                local.push(compact(raw(condition, self.body)));
                self.visit(body, &mut env.clone(), &local)?;
            }
        }
        if known == Some(true) {
            return Ok(());
        }
        let mut previous = vec![compact(raw(condition, self.body))];
        for alternative in children(node)
            .into_iter()
            .filter(|n| matches!(n.kind(), "else_statement" | "elseif_statement"))
        {
            let mut local = scope.to_vec();
            if known.is_none() {
                local.push(format!("not ({})", previous.join(" or ")));
            }
            if alternative.kind() == "elseif_statement" {
                let condition = field(alternative, "condition");
                let truth = eval(condition, self.body, env).truth();
                if truth != Some(false)
                    && let Some(body) = alternative.child_by_field_name("consequence")
                {
                    if truth.is_none() {
                        local.push(compact(raw(condition, self.body)));
                    }
                    self.visit(body, &mut env.clone(), &local)?;
                }
                if truth == Some(true) {
                    break;
                }
                previous.push(compact(raw(condition, self.body)));
            } else if let Some(body) = alternative.child_by_field_name("body") {
                if known == Some(false) {
                    self.visit(body, env, &local)?;
                } else {
                    self.visit(body, &mut env.clone(), &local)?;
                }
            }
        }
        Ok(())
    }

    fn loop_body(&mut self, node: Node<'_>, env: &mut Env, scope: &[String]) -> Result<(), String> {
        let clause = field(node, "clause");
        let Some(body) = node.child_by_field_name("body") else {
            return Ok(());
        };
        let mut iterations = Vec::new();
        if clause.kind() == "for_numeric_clause" {
            let name = raw(field(clause, "name"), self.body);
            let start = eval(field(clause, "start"), self.body, env);
            let end = eval(field(clause, "end"), self.body, env);
            let step = clause
                .child_by_field_name("step")
                .map(|n| eval(n, self.body, env))
                .unwrap_or(Value::Number(1));
            if let (Value::Number(mut i), Value::Number(end), Value::Number(step)) =
                (start, end, step)
            {
                if step == 0 {
                    return Err(format!("{}: zero loop step", name_line(node)));
                }
                while if step > 0 { i <= end } else { i >= end } {
                    if iterations.len() >= 1024 {
                        return Err(format!("{}: loop exceeds 1024 iterations", name_line(node)));
                    }
                    let mut local = env.clone();
                    local.insert(name.into(), Value::Number(i));
                    iterations.push(local);
                    let Some(next) = i.checked_add(step) else {
                        break;
                    };
                    i = next;
                }
                for mut local in iterations {
                    self.visit(body, &mut local, scope)?;
                }
                return Ok(());
            }
        } else {
            let nodes = children(clause);
            let names = nodes
                .iter()
                .find(|n| n.kind() == "variable_list")
                .map(|n| children(*n))
                .unwrap_or_default();
            let expressions = nodes
                .iter()
                .find(|n| n.kind() == "expression_list")
                .map(|n| children(*n))
                .unwrap_or_default();
            if let Some(call) = expressions.first().filter(|n| n.kind() == "function_call") {
                let name = raw(field(*call, "name"), self.body);
                let args = children(field(*call, "arguments"));
                if matches!(name, "pairs" | "ipairs")
                    && args.len() == 1
                    && let Value::Table(entries) = eval(args[0], self.body, env)
                {
                    for (key, value) in entries {
                        let mut local = env.clone();
                        if let Some(n) = names.first() {
                            local.insert(
                                raw(*n, self.body).into(),
                                key.parse().map(Value::Number).unwrap_or(Value::String(key)),
                            );
                        }
                        if let Some(n) = names.get(1) {
                            local.insert(raw(*n, self.body).into(), value);
                        }
                        self.visit(body, &mut local, scope)?;
                    }
                    return Ok(());
                }
            }
        }
        let mut local = scope.to_vec();
        local.push(format!("for {}", compact(raw(clause, self.body))));
        self.visit(body, &mut env.clone(), &local)
    }
}

fn name_line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

pub(crate) fn describe(action: &str) -> String {
    let action = action
        .strip_prefix("act.")
        .or_else(|| action.strip_prefix("wezterm.action."))
        .unwrap_or(action);
    if let Some((name, args)) = action.split_once('(') {
        let args = args.trim_end_matches(')');
        if args.len() < 65 && !args.contains(['{', '(']) {
            return format!("{}: {}", humanize(name), args.trim_matches('"'));
        }
        return humanize(name);
    }
    humanize(action)
}

fn module_active(body: &str, module: &str, env: &Env) -> Result<bool, String> {
    let syntax = tree(body)?;
    let mut alias = None;
    for declaration in children(syntax.root_node())
        .into_iter()
        .filter(|n| n.kind() == "variable_declaration")
    {
        let Some(assignment) = children(declaration)
            .into_iter()
            .find(|n| n.kind() == "assignment_statement")
        else {
            continue;
        };
        let nodes = children(assignment);
        let names = nodes
            .iter()
            .find(|n| n.kind() == "variable_list")
            .map(|n| children(*n))
            .unwrap_or_default();
        let values = nodes
            .iter()
            .find(|n| n.kind() == "expression_list")
            .map(|n| children(*n))
            .unwrap_or_default();
        for (name, value) in names.iter().zip(values) {
            if value.kind() == "function_call" && raw(field(value, "name"), body) == "require" {
                let args = children(field(value, "arguments"));
                if args
                    .first()
                    .is_some_and(|n| eval(*n, body, env).text() == module)
                {
                    alias = Some(raw(*name, body).to_string());
                }
            }
        }
    }
    let Some(alias) = alias else {
        return Ok(true);
    };
    fn used(node: Node<'_>, body: &str, alias: &str, env: &Env) -> bool {
        if node.kind() == "identifier" && raw(node, body) == alias {
            return true;
        }
        if node.kind() == "variable_declaration"
            && raw(node, body).contains("require")
            && raw(node, body).contains(alias)
        {
            return false;
        }
        if node.kind() == "if_statement" {
            let truth = eval(field(node, "condition"), body, env).truth();
            if truth != Some(false)
                && node
                    .child_by_field_name("consequence")
                    .is_some_and(|n| used(n, body, alias, env))
            {
                return true;
            }
            return truth != Some(true)
                && children(node)
                    .into_iter()
                    .filter(|n| matches!(n.kind(), "else_statement" | "elseif_statement"))
                    .any(|n| used(n, body, alias, env));
        }
        children(node)
            .into_iter()
            .any(|n| used(n, body, alias, env))
    }
    Ok(used(syntax.root_node(), body, &alias, env))
}

pub fn parse(
    source: &str,
    body: &str,
    package: &str,
    sources: &BTreeMap<String, (&str, String)>,
    rows: &mut Package,
) -> Result<(), String> {
    let syntax = tree(body)?;
    let variants: &[(&str, bool)] = if package == "wezterm" {
        &[("macOS", true), ("Linux/Windows", false)]
    } else {
        &[("", false)]
    };
    for (platform, mac) in variants {
        let mut env = Env::new();
        if package == "wezterm" {
            env.insert("platform.is_mac".into(), Value::Bool(*mac));
            if let Some((base, relative)) = source.split_once("/wezterm/") {
                let prefix = format!("{base}/wezterm/");
                if let Some((_, main)) = sources.get(&(prefix.clone() + "keymap/init.lua")) {
                    let module = relative.trim_end_matches(".lua").replace('/', ".");
                    if !module_active(main, &module, &env)? {
                        continue;
                    }
                }
                if let Some((_, modifiers)) = sources.get(&(prefix + "keymap/modifiers.lua")) {
                    let modifiers_tree = tree(modifiers)?;
                    let mut ignored = Package::default();
                    let mut scan = Scan {
                        source,
                        body: modifiers,
                        package,
                        rows: &mut ignored,
                    };
                    scan.visit(modifiers_tree.root_node(), &mut env, &[])?;
                    if let Some(modifiers) = env.get("MOD").cloned() {
                        env.insert("require:keymap.modifiers".into(), modifiers);
                    }
                }
            }
        }
        let mut scope = vec![platform.to_string()];
        if package == "nvim"
            && let Some((_, file)) = source.split_once("/lua/plugins/")
        {
            scope.push(file.trim_end_matches(".lua").to_string());
        }
        let mut scan = Scan {
            source,
            body,
            package,
            rows,
        };
        scan.visit(syntax.root_node(), &mut env, &scope)?;
    }
    Ok(())
}
