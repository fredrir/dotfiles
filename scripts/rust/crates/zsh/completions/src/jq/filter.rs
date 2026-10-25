use super::lexer::{self, Kind, Part, Token};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    Nothing,
    // `.na`
    Field,
    // `."na`
    QuotedField,
    // `["na`
    Index,
    // `{na`
    ObjectKey,
    // A new term: `.key`, functions and keywords.
    Term,
    // After a complete term, where only operators spelled as words fit.
    Operator,
    Variable,
    Format,
    Module { data: bool },
    ModuleFunction { module: String },
}

// The filter with a hole at the cursor: `before`, then the probe, then `after`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub path: String,
    pub alias: Option<String>,
    pub data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub want: Want,
    // Bytes of the filter before the text the candidates replace.
    pub skip: usize,
    pub partial: String,
    // What the keys are read from, such as `.items[]` in `.items[].na`.
    pub base: String,
    pub program: Option<Program>,
    pub keywords: Vec<&'static str>,
    pub variables: Vec<String>,
    pub functions: Vec<(String, usize)>,
    pub imports: Vec<Import>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Frame {
    Paren,
    Bracket,
    Brace {
        key: bool,
    },
    Interpolation,
    If(Branch),
    Reduce {
        bound: bool,
    },
    Def {
        name: String,
        params: bool,
        body: bool,
    },
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    Condition,
    Then,
    Else,
}

const TERM_KEYWORDS: [&str; 6] = ["if", "try", "reduce", "foreach", "def", "label"];
const BINDERS: [&str; 5] = ["as", "def", "label", "import", "include"];

pub fn analyze(text: &str) -> Analysis {
    let tokens = lexer::lex(text);
    let (functions, imports) = definitions(text, &tokens);
    let mut analysis = Analysis {
        want: Want::Nothing,
        skip: text.len(),
        partial: String::new(),
        base: String::new(),
        program: None,
        keywords: Vec::new(),
        variables: variables(&tokens),
        functions,
        imports,
    };
    let touching = tokens.last().filter(|token| token.end == text.len());
    let Some(last) = touching else {
        position(text, &tokens, tokens.len(), &mut analysis);
        return analysis;
    };
    let index = tokens.len() - 1;
    let previous = index.checked_sub(1).map(|at| &tokens[at]);
    match &last.kind {
        Kind::Comment | Kind::Num | Kind::Recurse => {}
        Kind::Str(part) if last.open => {
            analysis.skip = last.start + 1;
            analysis.partial = text[last.start + 1..].to_string();
            if *part != Part::Whole {
                return analysis;
            }
            match previous {
                Some(dot) if dot.kind == Kind::Dot && dot.end == last.start => {
                    analysis.want = Want::QuotedField;
                    path(text, &tokens, index - 1, &mut analysis);
                }
                Some(bracket)
                    if bracket.is_punct("[") && index >= 2 && ends_term_at(&tokens, index - 2) =>
                {
                    analysis.want = Want::Index;
                    path(text, &tokens, index - 1, &mut analysis);
                }
                Some(directive)
                    if directive.is_ident("import") || directive.is_ident("include") =>
                {
                    analysis.want = Want::Module {
                        data: directive.is_ident("import"),
                    };
                }
                _ => {}
            }
        }
        Kind::Field(name) => {
            analysis.want = Want::Field;
            analysis.skip = last.start + 1;
            analysis.partial = name.clone();
            path(text, &tokens, index, &mut analysis);
        }
        Kind::Dot => {
            analysis.want = Want::Field;
            analysis.skip = last.end;
            path(text, &tokens, index, &mut analysis);
        }
        Kind::Var(name) => {
            let binding = previous.is_some_and(|token| token.is_ident("label"))
                || in_parameters(&tokens[..index])
                || in_pattern(&tokens[..index]);
            if !binding {
                analysis.want = Want::Variable;
                analysis.skip = last.start;
                analysis.partial = format!("${name}");
            }
        }
        Kind::Format(name) => {
            analysis.want = Want::Format;
            analysis.skip = last.start;
            analysis.partial = format!("@{name}");
        }
        Kind::Ident(name) => {
            if let Some((module, _)) = name.rsplit_once("::") {
                analysis.want = Want::ModuleFunction {
                    module: module.to_string(),
                };
                analysis.skip = last.start;
                analysis.partial = name.clone();
            } else {
                position(text, &tokens, index, &mut analysis);
                analysis.skip = last.start;
                analysis.partial = name.clone();
                if analysis.want == Want::Term {
                    analysis.program = None;
                }
            }
        }
        Kind::Str(_) | Kind::Punct(_) => position(text, &tokens, tokens.len(), &mut analysis),
    }
    analysis
}

// What starts at `tokens[index]`, which is either the word being typed or the end of the filter.
fn position(text: &str, tokens: &[Token], index: usize, analysis: &mut Analysis) {
    let before = &tokens[..index];
    let previous = before.last();
    if previous.is_some_and(|token| BINDERS.iter().any(|binder| token.is_ident(binder)))
        || in_parameters(before)
        || in_pattern(before)
    {
        return;
    }
    let frames = frames(before);
    let start = tokens.get(index).map_or(text.len(), |token| token.start);
    if index > 0 && ends_term_at(tokens, index - 1) {
        analysis.want = Want::Operator;
        analysis.keywords = operator_keywords(before, &frames);
        return;
    }
    if matches!(frames.last(), Some(Frame::Brace { key: true })) {
        analysis.want = Want::ObjectKey;
        analysis.program = Some(Program {
            before: format!("{}(", &text[..start]),
            after: format!("): null{}", closers(&frames)),
        });
        return;
    }
    analysis.want = Want::Term;
    analysis.keywords = TERM_KEYWORDS.to_vec();
    if at_program_start(before, &frames) {
        analysis.keywords.extend(["import", "include"]);
    }
    analysis.program = Some(Program {
        before: text[..start].to_string(),
        after: closers(&frames),
    });
}

// Keys are read from what comes before the dot or bracket at `unit`, in the context around it.
fn path(text: &str, tokens: &[Token], unit: usize, analysis: &mut Analysis) {
    let chained = unit > 0 && ends_term_at(tokens, unit - 1);
    let start = if chained {
        chain_start(tokens, unit - 1)
    } else {
        unit
    };
    let mut base = text[tokens[start].start..tokens[unit].start].trim_end();
    // `.a.[` and `.a."b"` continue `.a`; a lone `.` before `[` is the input itself.
    if base.len() > 1 && base.ends_with('.') && !base.ends_with("..") {
        base = &base[..base.len() - 1];
    }
    let frames = frames(&tokens[..start]);
    let prefix = &text[..tokens[start].start];
    analysis.base = base.to_string();
    analysis.program = Some(if base.is_empty() {
        Program {
            before: prefix.to_string(),
            after: closers(&frames),
        }
    } else {
        Program {
            before: format!("{prefix}({base} | "),
            after: format!("){}", closers(&frames)),
        }
    });
}

fn ends_term_at(tokens: &[Token], index: usize) -> bool {
    let token = &tokens[index];
    token.ends_term() || token.is_ident("end")
}

// The first token of the term ending at `index`, following postfixes such as `.a`, `[0]` and `?`.
fn chain_start(tokens: &[Token], mut index: usize) -> usize {
    loop {
        let token = &tokens[index];
        let chained = |at: usize| at > 0 && ends_term_at(tokens, at - 1);
        match &token.kind {
            Kind::Punct(")") => {
                let open = matching(tokens, index, |t| t.is_punct("("), |t| t.is_punct(")"));
                let call = open > 0
                    && tokens[open - 1]
                        .ident()
                        .is_some_and(|name| !lexer::is_keyword(name));
                return if call { open - 1 } else { open };
            }
            Kind::Punct("]") => {
                let open = matching(tokens, index, |t| t.is_punct("["), |t| t.is_punct("]"));
                if !chained(open) {
                    return open;
                }
                index = open - 1;
            }
            Kind::Punct("}") => {
                return matching(tokens, index, |t| t.is_punct("{"), |t| t.is_punct("}"));
            }
            Kind::Punct("?") | Kind::Field(_) if chained(index) => index -= 1,
            Kind::Dot
                if chained(index) && tokens.get(index + 1).is_some_and(|t| t.is_punct("[")) =>
            {
                index -= 1;
            }
            Kind::Str(part @ (Part::Whole | Part::End)) => {
                let open = if *part == Part::End {
                    matching(
                        tokens,
                        index,
                        |t| t.kind == Kind::Str(Part::Start),
                        |t| t.kind == Kind::Str(Part::End),
                    )
                } else {
                    index
                };
                let dot = open.checked_sub(1).filter(|at| {
                    tokens[*at].kind == Kind::Dot && tokens[*at].end == tokens[open].start
                });
                match dot {
                    Some(dot) if chained(dot) => index = dot - 1,
                    Some(dot) => return dot,
                    None if open > 0 && matches!(tokens[open - 1].kind, Kind::Format(_)) => {
                        return open - 1;
                    }
                    None => return open,
                }
            }
            Kind::Ident(name) if name == "end" => {
                return matching(tokens, index, |t| t.is_ident("if"), |t| t.is_ident("end"));
            }
            _ => return index,
        }
    }
}

// The opener paired with the closer at `close`, or the first token when it is missing.
fn matching(
    tokens: &[Token],
    close: usize,
    opens: impl Fn(&Token) -> bool,
    closes: impl Fn(&Token) -> bool,
) -> usize {
    let mut depth = 0usize;
    for at in (0..=close).rev() {
        if closes(&tokens[at]) {
            depth += 1;
        } else if opens(&tokens[at]) {
            depth -= 1;
            if depth == 0 {
                return at;
            }
        }
    }
    0
}

fn frames(tokens: &[Token]) -> Vec<Frame> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut previous: Option<&Token> = None;
    for token in tokens {
        match &token.kind {
            Kind::Punct("(") => {
                match stack.last_mut() {
                    Some(Frame::Reduce { bound: true }) => {
                        stack.pop();
                    }
                    Some(Frame::Def {
                        body: false,
                        params,
                        ..
                    }) => *params = true,
                    _ => {}
                }
                stack.push(Frame::Paren);
            }
            Kind::Punct(")") => close(&mut stack, |frame| *frame == Frame::Paren),
            Kind::Punct("[") => stack.push(Frame::Bracket),
            Kind::Punct("]") => close(&mut stack, |frame| *frame == Frame::Bracket),
            Kind::Punct("{") => stack.push(Frame::Brace { key: true }),
            Kind::Punct("}") => close(&mut stack, |frame| matches!(frame, Frame::Brace { .. })),
            Kind::Punct(":") => match stack.last_mut() {
                Some(Frame::Brace { key }) => *key = false,
                Some(Frame::Def { body, .. }) => *body = true,
                _ => {}
            },
            Kind::Punct(",") => {
                if let Some(Frame::Brace { key }) = stack.last_mut() {
                    *key = true;
                }
            }
            Kind::Punct(";") => {
                if matches!(
                    stack.last(),
                    Some(Frame::Def { body: true, .. } | Frame::Import)
                ) {
                    stack.pop();
                }
            }
            Kind::Str(Part::Start) => stack.push(Frame::Interpolation),
            Kind::Str(Part::End) => close(&mut stack, |frame| *frame == Frame::Interpolation),
            Kind::Ident(name) => match name.as_str() {
                "if" => stack.push(Frame::If(Branch::Condition)),
                "then" | "elif" | "else" => {
                    if let Some(Frame::If(branch)) = stack.last_mut() {
                        *branch = match name.as_str() {
                            "then" => Branch::Then,
                            "elif" => Branch::Condition,
                            _ => Branch::Else,
                        };
                    }
                }
                "end" => close(&mut stack, |frame| matches!(frame, Frame::If(_))),
                "reduce" | "foreach" => stack.push(Frame::Reduce { bound: false }),
                "as" => {
                    if let Some(Frame::Reduce { bound }) = stack.last_mut() {
                        *bound = true;
                    }
                }
                "def" => stack.push(Frame::Def {
                    name: String::new(),
                    params: false,
                    body: false,
                }),
                "import" | "include" => stack.push(Frame::Import),
                _ => {
                    if previous.is_some_and(|token| token.is_ident("def"))
                        && let Some(Frame::Def { name: defined, .. }) = stack.last_mut()
                    {
                        defined.clone_from(name);
                    }
                }
            },
            _ => {}
        }
        previous = Some(token);
    }
    stack
}

fn close(stack: &mut Vec<Frame>, opened_by: impl Fn(&Frame) -> bool) {
    if let Some(at) = stack.iter().rposition(opened_by) {
        stack.truncate(at);
    }
}

// Text that completes every construct still open, so the filter compiles.
fn closers(frames: &[Frame]) -> String {
    let mut text = String::new();
    for frame in frames.iter().rev() {
        match frame {
            Frame::Paren => text.push(')'),
            Frame::Bracket => text.push(']'),
            Frame::Brace { .. } => text.push('}'),
            Frame::Interpolation => text.push_str(")\""),
            Frame::If(Branch::Condition) => text.push_str(" then empty else empty end"),
            Frame::If(Branch::Then) => text.push_str(" else empty end"),
            Frame::If(Branch::Else) => text.push_str(" end"),
            Frame::Reduce { bound: false } => text.push_str(" as $__zcomp (.; .)"),
            // A definition is only run when called; one without parameters is called right away.
            Frame::Def {
                name,
                params: false,
                body: true,
            } if !name.is_empty() => text.push_str(&format!("; {name}")),
            Frame::Def { body: true, .. } => text.push_str("; ."),
            Frame::Reduce { bound: true } | Frame::Def { .. } | Frame::Import => {}
        }
    }
    text
}

fn operator_keywords(tokens: &[Token], frames: &[Frame]) -> Vec<&'static str> {
    let mut keywords = vec!["and", "or", "as"];
    match frames.last() {
        Some(Frame::If(Branch::Condition)) => keywords.push("then"),
        Some(Frame::If(Branch::Then)) => keywords.extend(["elif", "else", "end"]),
        Some(Frame::If(Branch::Else)) => keywords.push("end"),
        _ => {}
    }
    let count = |name: &str| tokens.iter().filter(|token| token.is_ident(name)).count();
    if count("try") > count("catch") {
        keywords.push("catch");
    }
    keywords
}

// Only module directives come before the cursor.
fn at_program_start(tokens: &[Token], frames: &[Frame]) -> bool {
    tokens.is_empty()
        || (frames.is_empty()
            && tokens.last().is_some_and(|token| token.is_punct(";"))
            && tokens
                .first()
                .is_some_and(|token| token.is_ident("import") || token.is_ident("include")))
}

// The cursor sits in the parameter list of `def name(`.
fn in_parameters(tokens: &[Token]) -> bool {
    let Some(open) = tokens.iter().rposition(|token| token.is_punct("(")) else {
        return false;
    };
    let closed = tokens[open..].iter().any(|token| token.is_punct(")"));
    !closed && open >= 2 && tokens[open - 2].is_ident("def")
}

// The cursor follows `as` before the pattern ends at `|`, `;` or the `(` of `reduce`.
fn in_pattern(tokens: &[Token]) -> bool {
    tokens
        .iter()
        .rposition(|token| token.is_ident("as"))
        .is_some_and(|at| {
            !tokens[at..]
                .iter()
                .any(|token| token.is_punct("|") || token.is_punct(";") || token.is_punct("("))
        })
}

// Variables the filter reads but never binds, such as `$name` for `--arg name`.
pub fn free_variables(text: &str) -> Vec<String> {
    let tokens = lexer::lex(text);
    let bound = variables(&tokens);
    let mut free: Vec<String> = Vec::new();
    for token in &tokens {
        let Kind::Var(name) = &token.kind else {
            continue;
        };
        let builtin = matches!(
            name.as_str(),
            "" | "ENV" | "ARGS" | "__loc__" | "__prog_args"
        );
        if !builtin
            && !name.contains("::")
            && !bound.contains(&format!("${name}"))
            && !free.contains(name)
        {
            free.push(name.clone());
        }
    }
    free
}

// Variables bound before the cursor: `as` patterns, parameters, labels and data imports.
fn variables(tokens: &[Token]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |name: String| {
        if !found.contains(&name) {
            found.push(name);
        }
    };
    for (at, token) in tokens.iter().enumerate() {
        let next = tokens.get(at + 1);
        match token.ident() {
            Some("as") => {
                for pattern in &tokens[at + 1..] {
                    match &pattern.kind {
                        Kind::Var(name) if !name.is_empty() => add(format!("${name}")),
                        Kind::Var(_) | Kind::Ident(_) | Kind::Str(Part::Whole) => {}
                        Kind::Punct("[" | "]" | "{" | "}" | "," | ":" | "?//") => {}
                        _ => break,
                    }
                }
            }
            Some("label") => {
                if let Some(Kind::Var(name)) = next.map(|token| &token.kind) {
                    add(format!("${name}"));
                }
            }
            Some("def") => {
                let params = tokens[at + 1..]
                    .iter()
                    .skip(1)
                    .take_while(|token| !token.is_punct(":") && !token.is_punct(")"));
                for param in params {
                    if let Kind::Var(name) = &param.kind {
                        add(format!("${name}"));
                    }
                }
            }
            _ => {}
        }
    }
    found
}

// Functions defined in the filter, with their arity, and its module directives.
pub fn definitions(text: &str, tokens: &[Token]) -> (Vec<(String, usize)>, Vec<Import>) {
    let mut functions = Vec::new();
    let mut imports = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        let next = tokens.get(at + 1);
        if token.is_ident("def")
            && let Some(name) = next.and_then(Token::ident)
        {
            let mut arity = 0;
            if tokens.get(at + 2).is_some_and(|token| token.is_punct("(")) {
                arity = 1;
                let mut depth = 0usize;
                for param in &tokens[at + 2..] {
                    match &param.kind {
                        Kind::Punct("(") => depth += 1,
                        Kind::Punct(")") => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Kind::Punct(";") if depth == 1 => arity += 1,
                        _ => {}
                    }
                }
            }
            if !functions
                .iter()
                .any(|(known, n)| known == name && *n == arity)
            {
                functions.push((name.to_string(), arity));
            }
        }
        let directive = token.is_ident("import") || token.is_ident("include");
        if directive
            && let Some(path) =
                next.filter(|next| next.kind == Kind::Str(Part::Whole) && !next.open)
        {
            let alias = tokens
                .get(at + 2)
                .filter(|token| token.is_ident("as"))
                .and_then(|_| tokens.get(at + 3));
            let (alias, data) = match alias.map(|token| &token.kind) {
                Some(Kind::Ident(name)) => (Some(name.clone()), false),
                Some(Kind::Var(name)) => (Some(name.clone()), true),
                _ => (None, false),
            };
            imports.push(Import {
                path: serde_json::from_str(&text[path.start..path.end]).unwrap_or_default(),
                alias,
                data,
            });
        }
    }
    (functions, imports)
}

#[cfg(test)]
#[path = "../../tests/unit/jq/filter_tests.rs"]
mod tests;
