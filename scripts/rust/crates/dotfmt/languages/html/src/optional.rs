use std::borrow::Cow;

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

const RAW: &[&str] = &["script", "style", "textarea", "title"];

/// Elements whose end tag HTML5 lets authors omit.
const OPTIONAL: &[&str] = &[
    "body", "colgroup", "dd", "dt", "head", "html", "li", "optgroup", "option", "p", "rp", "rt",
    "tbody", "td", "tfoot", "th", "thead", "tr",
];

/// Start tags that may appear while `head` is still open.
const HEAD_CONTENT: &[&str] = &[
    "base", "basefont", "bgsound", "link", "meta", "noframes", "noscript", "script", "style",
    "template", "title",
];

/// Start tags that implicitly close an open `p`.
const BLOCK_CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "plaintext",
    "pre",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
    "xmp",
];

/// Insert the end tags that HTML5 makes optional, so documents relying on the
/// omitted tags parse.
///
/// Returns the input unchanged when the omitted tags cannot be repaired without
/// inventing a content model, which keeps the original syntax error intact for
/// genuinely malformed input.
pub fn insert_optional_end_tags(input: &str) -> Cow<'_, str> {
    repair(input).map_or(Cow::Borrowed(input), Cow::Owned)
}

fn repair(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len() + 32);
    let mut stack: Vec<String> = Vec::new();
    let mut index = 0;
    let mut changed = false;
    while index < input.len() {
        let rest = &input[index..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |offset| offset + 3);
            output.push_str(&rest[..end]);
            index += end;
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            let end = rest.find('>').map_or(rest.len(), |offset| offset + 1);
            output.push_str(&rest[..end]);
            index += end;
            continue;
        }
        if rest.starts_with("</") {
            let (name, end) = tag_name(rest, 2)?;
            let name = name.to_ascii_lowercase();
            if let Some(position) = stack.iter().rposition(|open| *open == name) {
                while stack.len() > position + 1 {
                    let open = stack.pop().expect("stack entry");
                    if !OPTIONAL.contains(&open.as_str()) {
                        return None;
                    }
                    end_tag(&mut output, &open);
                    changed = true;
                }
                stack.pop();
            } else if !stack.is_empty() {
                // Nothing open matches this end tag, so closing anything would
                // invent a content model. Leave the document to the parser.
                return None;
            }
            output.push_str(&rest[..end]);
            index += end;
            continue;
        }
        if rest.starts_with('<') {
            let (name, end) = tag_name(rest, 1)?;
            let lower = name.to_ascii_lowercase();
            while let Some(open) = stack.last() {
                if !closes_on_start(open, &lower) {
                    break;
                }
                let open = stack.pop().expect("stack entry");
                end_tag(&mut output, &open);
                changed = true;
            }
            output.push_str(&rest[..end]);
            index += end;
            if !VOID.contains(&lower.as_str()) && !rest[..end].trim_end().ends_with("/>") {
                if RAW.contains(&lower.as_str())
                    && let Some(close) = find_raw_close(&input[index..], &lower)
                {
                    output.push_str(&input[index..index + close]);
                    index += close;
                }
                stack.push(lower);
            }
            continue;
        }
        let character = rest.chars().next().expect("character");
        output.push(character);
        index += character.len_utf8();
    }
    while let Some(open) = stack.pop() {
        if !OPTIONAL.contains(&open.as_str()) {
            return None;
        }
        end_tag(&mut output, &open);
        changed = true;
    }
    changed.then_some(output)
}

fn end_tag(output: &mut String, name: &str) {
    output.push_str("</");
    output.push_str(name);
    output.push_str(">\n");
}

fn tag_name(input: &str, offset: usize) -> Option<(&str, usize)> {
    let bytes = input.as_bytes();
    let mut index = offset;
    while index < bytes.len() && (bytes[index] as char).is_ascii_whitespace() {
        index += 1;
    }
    let start = index;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':') {
            index += 1;
        } else {
            break;
        }
    }
    if index == start {
        return None;
    }
    let name_end = index;
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(current) = quote {
            if byte == current {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some((&input[start..name_end], index + 1));
        }
        index += 1;
    }
    None
}

fn find_raw_close(input: &str, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    input
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

/// Whether `new` implicitly closes the open element `open`, per the HTML5
/// "generate implied end tags" and table insertion rules. Both names are
/// lowercase.
fn closes_on_start(open: &str, new: &str) -> bool {
    match open {
        "head" => !HEAD_CONTENT.contains(&new) && !matches!(new, "head" | "html"),
        "li" => new == "li",
        "dt" | "dd" => matches!(new, "dt" | "dd"),
        "p" => BLOCK_CLOSES_P.contains(&new),
        "option" => matches!(new, "option" | "optgroup"),
        "optgroup" => new == "optgroup",
        "rt" | "rp" => matches!(new, "rt" | "rp"),
        "colgroup" => matches!(
            new,
            "colgroup" | "caption" | "thead" | "tbody" | "tfoot" | "tr"
        ),
        "thead" | "tbody" | "tfoot" => {
            matches!(new, "caption" | "colgroup" | "thead" | "tbody" | "tfoot")
        }
        "tr" => matches!(
            new,
            "caption" | "colgroup" | "tr" | "thead" | "tbody" | "tfoot"
        ),
        "td" | "th" => matches!(
            new,
            "td" | "th" | "tr" | "caption" | "colgroup" | "thead" | "tbody" | "tfoot"
        ),
        _ => false,
    }
}
