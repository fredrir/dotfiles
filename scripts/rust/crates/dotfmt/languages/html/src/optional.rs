
use std::borrow::Cow;

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

const RAW: &[&str] = &["script", "style", "textarea", "title"];

const OPTIONAL: &[&str] = &[
    "body", "colgroup", "dd", "dt", "head", "html", "li", "optgroup", "option", "p", "rp", "rt",
    "tbody", "td", "tfoot", "th", "thead", "tr",
];

const CONTAINER: &[&str] = &[
    "body", "div", "dl", "html", "menu", "ol", "optgroup", "section", "select", "table", "tbody",
    "tfoot", "thead", "tr", "ul",
];

const BLOCK_CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "dialog",
    "div",
    "dl",
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
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "search",
    "section",
    "table",
    "ul",
];

pub fn insert_optional_end_tags(input: &str) -> Cow<'_, str> {
    let mut output = String::with_capacity(input.len() + 32);
    let mut stack: Vec<String> = Vec::new();
    let mut index = 0;
    let mut changed = false;
    while index < input.len() {
        let rest = &input[index..];
        if rest.starts_with("<!--") {
            let end = rest
                .find("-->")
                .map_or(input.len() - index, |offset| offset + 3);
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
            let Some((name, end)) = tag_name(rest, 2) else {
                output.push_str(rest);
                break;
            };
            let name = name.to_ascii_lowercase();
            if let Some(position) = stack
                .iter()
                .rposition(|open| open.eq_ignore_ascii_case(&name))
            {
                while stack.len() > position + 1 {
                    let open = stack.pop().expect("stack entry");
                    output.push_str("</");
                    output.push_str(&open);
                    output.push_str(">\n");
                    changed = true;
                }
                stack.pop();
            } else if CONTAINER.contains(&name.as_str()) {
                while let Some(open) = stack.pop() {
                    output.push_str("</");
                    output.push_str(&open);
                    output.push_str(">\n");
                    changed = true;
                }
            }
            output.push_str(&rest[..end]);
            index += end;
            continue;
        }
        if rest.starts_with('<') {
            let Some((name, end)) = tag_name(rest, 1) else {
                output.push_str(rest);
                break;
            };
            let lower = name.to_ascii_lowercase();
            while let Some(open) = stack.last() {
                if !closes_on_start(open, &lower) {
                    break;
                }
                let open = stack.pop().expect("stack entry");
                output.push_str("</");
                output.push_str(&open);
                output.push_str(">\n");
                changed = true;
            }
            let tag = &rest[..end];
            output.push_str(tag);
            index += end;
            if !VOID.contains(&lower.as_str()) && !tag.trim_end().ends_with("/>") {
                if RAW.contains(&lower.as_str()) {
                    if let Some(close) = find_raw_close(&input[index..], &lower) {
                        output.push_str(&input[index..index + close]);
                        index += close;
                    }
                } else if OPTIONAL.contains(&lower.as_str()) {
                    stack.push(name.to_owned());
                }
            }
            continue;
        }
        let character = rest.chars().next().expect("character");
        output.push(character);
        index += character.len_utf8();
    }
    while let Some(open) = stack.pop() {
        output.push_str("</");
        output.push_str(&open);
        output.push_str(">\n");
        changed = true;
    }
    if changed {
        Cow::Owned(output)
    } else {
        Cow::Borrowed(input)
    }
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

fn closes_on_start(open: &str, new: &str) -> bool {
    match open.to_ascii_lowercase().as_str() {
        "li" => new == "li",
        "p" => BLOCK_CLOSES_P.contains(&new),
        "dt" | "dd" => matches!(new, "dt" | "dd"),
        "tr" => new == "tr",
        "td" | "th" => matches!(new, "td" | "th" | "tr"),
        "thead" | "tbody" | "tfoot" => matches!(new, "thead" | "tbody" | "tfoot"),
        "option" => matches!(new, "option" | "optgroup"),
        "optgroup" => new == "optgroup",
        "colgroup" => new == "colgroup",
        "rt" | "rp" => matches!(new, "rt" | "rp"),
        _ => false,
    }
}
