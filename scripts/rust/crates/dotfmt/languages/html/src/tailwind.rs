use dotfmt_tailwind::Sorter;

const DEFAULT_CLASS_ATTRIBUTES: [&str; 2] = ["class", "className"];

struct Replacement {
    start: usize,
    end: usize,
    value: String,
}

pub(crate) fn sort_class_attributes(
    markup: &str,
    sorter: &Sorter,
    configured: &[String],
) -> String {
    if file_is_ignored(markup) {
        return markup.to_owned();
    }
    let mut replacements = Vec::new();
    scan(markup, sorter, configured, &mut replacements);
    if replacements.is_empty() {
        return markup.to_owned();
    }
    let mut output = String::with_capacity(markup.len());
    let mut cursor = 0;
    for replacement in replacements {
        if replacement.start < cursor {
            continue;
        }
        output.push_str(&markup[cursor..replacement.start]);
        output.push_str(&replacement.value);
        cursor = replacement.end;
    }
    output.push_str(&markup[cursor..]);
    output
}

fn scan(markup: &str, sorter: &Sorter, configured: &[String], out: &mut Vec<Replacement>) {
    let bytes = markup.as_bytes();
    let mut offset = 0;
    while offset < markup.len() {
        if bytes[offset] != b'<' {
            offset = markup[offset..]
                .find('<')
                .map_or(markup.len(), |found| offset + found);
            continue;
        }
        if markup[offset..].starts_with("<!--") {
            let Some(end) = markup[offset..].find("-->").map(|found| offset + found + 3) else {
                break;
            };
            if has_ignore_directive(&markup[offset + 4..end - 3]) {
                let next = skip_whitespace(bytes, end);
                if next < markup.len()
                    && bytes[next] == b'<'
                    && !markup[next..].starts_with("</")
                    && !markup[next..].starts_with("<!")
                    && !markup[next..].starts_with("<?")
                {
                    offset = element_end(markup, next);
                    continue;
                }
            }
            offset = end;
            continue;
        }
        if markup[offset..].starts_with("<![CDATA[") {
            offset = markup[offset..]
                .find("]]>")
                .map_or(markup.len(), |found| offset + found + 3);
            continue;
        }
        if markup[offset..].starts_with("<!") || markup[offset..].starts_with("<?") {
            offset = tag_end(markup, offset);
            continue;
        }
        if offset + 1 < markup.len() && bytes[offset + 1] == b'/' {
            offset = tag_end(markup, offset);
            continue;
        }
        let tag = start_tag(markup, offset, Some((sorter, configured, out)));
        offset = tag.end;
        let name = &markup[tag.name_start..tag.name_end];
        if !tag.self_closing && is_raw_text_element(name) {
            offset = closing_tag_start(markup, offset, name);
        }
    }
}

struct StartTag {
    name_start: usize,
    name_end: usize,
    end: usize,
    self_closing: bool,
}

fn start_tag(
    markup: &str,
    start: usize,
    rewrite: Option<(&Sorter, &[String], &mut Vec<Replacement>)>,
) -> StartTag {
    let bytes = markup.as_bytes();
    let name_start = start + 1;
    let mut offset = name_start;
    while offset < markup.len() && !is_tag_name_end(bytes[offset]) {
        offset += 1;
    }
    let name_end = offset;
    let mut self_closing = false;
    let mut rewrite = rewrite;
    loop {
        offset = skip_whitespace(bytes, offset);
        if offset >= markup.len() {
            break;
        }
        match bytes[offset] {
            b'>' => {
                offset += 1;
                break;
            }
            b'/' if offset + 1 < markup.len() && bytes[offset + 1] == b'>' => {
                offset += 2;
                self_closing = true;
                break;
            }
            b'/' => offset += 1,
            b'{' => offset = skip_braced(markup, offset).unwrap_or(markup.len()),
            _ => {
                let attr_start = offset;
                while offset < markup.len() && !is_attribute_name_end(bytes[offset]) {
                    offset += 1;
                }
                let attr_name = &markup[attr_start..offset];
                if offset == attr_start {
                    offset += 1;
                    continue;
                }
                let after_name = offset;
                offset = skip_whitespace(bytes, offset);
                if offset < markup.len() && bytes[offset] == b'=' {
                    offset = skip_whitespace(bytes, offset + 1);
                    if offset >= markup.len() {
                        break;
                    }
                    let quote = bytes[offset];
                    if quote == b'"' || quote == b'\'' {
                        let value_start = offset + 1;
                        let Some(value_end) = find_quote(markup, value_start, quote) else {
                            break;
                        };
                        if let Some((sorter, configured, out)) = rewrite.as_mut() {
                            let value = &markup[value_start..value_end];
                            if let Some(sorted) =
                                rewrite_attribute(attr_name, value, sorter, configured)
                            {
                                out.push(Replacement {
                                    start: value_start,
                                    end: value_end,
                                    value: sorted,
                                });
                            }
                        }
                        offset = value_end + 1;
                    } else {
                        while offset < markup.len()
                            && !bytes[offset].is_ascii_whitespace()
                            && bytes[offset] != b'>'
                        {
                            if bytes[offset] == b'{' {
                                offset = skip_braced(markup, offset).unwrap_or(markup.len());
                            } else {
                                offset += 1;
                            }
                        }
                    }
                } else {
                    offset = after_name;
                }
            }
        }
    }
    StartTag {
        name_start,
        name_end,
        end: offset.min(markup.len()),
        self_closing,
    }
}

fn rewrite_attribute(
    name: &str,
    value: &str,
    sorter: &Sorter,
    configured: &[String],
) -> Option<String> {
    if is_vue_class_attribute(name) {
        return rewrite_vue_class(value, sorter);
    }
    if is_class_attribute(name, configured) {
        return rewrite_plain_class(value, sorter);
    }
    None
}

fn is_class_attribute(name: &str, configured: &[String]) -> bool {
    DEFAULT_CLASS_ATTRIBUTES
        .iter()
        .copied()
        .chain(configured.iter().map(String::as_str))
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

fn is_vue_class_attribute(name: &str) -> bool {
    name.eq_ignore_ascii_case(":class") || name.eq_ignore_ascii_case("v-bind:class")
}

fn rewrite_plain_class(value: &str, sorter: &Sorter) -> Option<String> {
    let trimmed = value.trim_ascii();
    if trimmed.is_empty() || has_dynamic_content(trimmed) {
        return None;
    }
    let sorted = sorter.sort_list(trimmed);
    if sorted.is_empty() || sorted == value {
        None
    } else {
        Some(sorted)
    }
}

fn rewrite_vue_class(value: &str, sorter: &Sorter) -> Option<String> {
    let start = value.len() - value.trim_ascii_start().len();
    let end = value.trim_ascii_end().len();
    if start >= end {
        return None;
    }
    let expression = &value[start..end];
    let sorted = rewrite_expression(expression, sorter)?;
    if sorted == expression {
        return None;
    }
    let mut output = String::with_capacity(value.len() + sorted.len());
    output.push_str(&value[..start]);
    output.push_str(&sorted);
    output.push_str(&value[end..]);
    Some(output)
}

fn rewrite_expression(expression: &str, sorter: &Sorter) -> Option<String> {
    if let Some((quote, inner)) = whole_string_literal(expression) {
        let sorted = sort_literal(inner, sorter)?;
        return Some(format!("{quote}{sorted}{quote}"));
    }
    if expression.starts_with('[') && expression.ends_with(']') {
        return rewrite_array(expression, sorter);
    }
    if expression.starts_with('{') && expression.ends_with('}') {
        return rewrite_object(expression, sorter);
    }
    None
}

fn rewrite_array(expression: &str, sorter: &Sorter) -> Option<String> {
    let inner_start = 1;
    let inner_end = expression.len() - 1;
    let segments = split_top_level(expression, inner_start, inner_end, b',');
    let mut changed = false;
    let mut output = String::with_capacity(expression.len());
    output.push('[');
    let mut cursor = inner_start;
    for (start, end) in segments {
        output.push_str(&expression[cursor..start]);
        let (value_start, value_end) = trim_bounds(expression, start, end);
        let value = &expression[value_start..value_end];
        if let Some(sorted) = rewrite_nested_value(value, sorter) {
            output.push_str(&expression[start..value_start]);
            output.push_str(&sorted);
            output.push_str(&expression[value_end..end]);
            changed = true;
        } else {
            output.push_str(&expression[start..end]);
        }
        cursor = end;
    }
    output.push_str(&expression[cursor..inner_end]);
    output.push(']');
    changed.then_some(output)
}

fn rewrite_object(expression: &str, sorter: &Sorter) -> Option<String> {
    let inner_start = 1;
    let inner_end = expression.len() - 1;
    let segments = split_top_level(expression, inner_start, inner_end, b',');
    let mut changed = false;
    let mut output = String::with_capacity(expression.len());
    output.push('{');
    let mut cursor = inner_start;
    for (start, end) in segments {
        output.push_str(&expression[cursor..start]);
        if let Some(colon) = find_top_level(expression, start, end, b':') {
            let (key_start, key_end) = trim_bounds(expression, start, colon);
            let key = &expression[key_start..key_end];
            if let Some((quote, inner)) = whole_string_literal(key)
                && let Some(sorted) = sort_literal(inner, sorter)
            {
                output.push_str(&expression[start..key_start]);
                output.push(quote);
                output.push_str(&sorted);
                output.push(quote);
                output.push_str(&expression[key_end..end]);
                changed = true;
                cursor = end;
                continue;
            }
        }
        output.push_str(&expression[start..end]);
        cursor = end;
    }
    output.push_str(&expression[cursor..inner_end]);
    output.push('}');
    changed.then_some(output)
}

fn rewrite_nested_value(value: &str, sorter: &Sorter) -> Option<String> {
    if let Some((quote, inner)) = whole_string_literal(value) {
        let sorted = sort_literal(inner, sorter)?;
        return Some(format!("{quote}{sorted}{quote}"));
    }
    if value.starts_with('[') && value.ends_with(']')
        || value.starts_with('{') && value.ends_with('}')
    {
        return rewrite_expression(value, sorter);
    }
    None
}

fn sort_literal(inner: &str, sorter: &Sorter) -> Option<String> {
    let trimmed = inner.trim_ascii();
    if trimmed.is_empty() || has_dynamic_content(trimmed) {
        return None;
    }
    let sorted = sorter.sort_list(trimmed);
    if sorted.is_empty() || sorted == inner {
        None
    } else {
        Some(sorted)
    }
}

fn whole_string_literal(value: &str) -> Option<(char, &str)> {
    let quote = value.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let bytes = value.as_bytes();
    let mut offset = 1;
    let mut escaped = false;
    while offset < bytes.len() {
        if escaped {
            escaped = false;
        } else if bytes[offset] == b'\\' {
            escaped = true;
        } else if bytes[offset] == quote as u8 {
            if offset + 1 == bytes.len() {
                return Some((quote, &value[1..offset]));
            }
            return None;
        }
        offset += 1;
    }
    None
}

fn has_dynamic_content(value: &str) -> bool {
    value.contains(['{', '}'])
        || value.contains("<%")
        || value.contains("%>")
        || value.contains("<!--")
        || value.contains("-->")
}

fn file_is_ignored(markup: &str) -> bool {
    let markup = markup.trim_ascii_start();
    let Some(rest) = markup.strip_prefix("<!--") else {
        return false;
    };
    let Some(end) = rest.find("-->") else {
        return false;
    };
    let comment = rest[..end].trim_ascii_start();
    comment.starts_with("prettier-ignore-file") || has_ignore_directive(comment)
}

fn has_ignore_directive(comment: &str) -> bool {
    comment
        .trim_ascii_start()
        .strip_prefix("prettier-ignore")
        .is_some_and(|rest| {
            rest.is_empty() || rest.starts_with(|character: char| character.is_ascii_whitespace())
        })
}

fn element_end(markup: &str, start: usize) -> usize {
    let bytes = markup.as_bytes();
    let first = start_tag(markup, start, None);
    let name = &markup[first.name_start..first.name_end];
    if first.self_closing || is_void_element(name) {
        return first.end;
    }
    let mut depth = 1usize;
    let mut offset = first.end;
    while offset < markup.len() {
        if bytes[offset] != b'<' {
            offset = markup[offset..]
                .find('<')
                .map_or(markup.len(), |found| offset + found);
            continue;
        }
        if markup[offset..].starts_with("<!--") {
            offset = markup[offset..]
                .find("-->")
                .map_or(markup.len(), |found| offset + found + 3);
            continue;
        }
        if markup[offset..].starts_with("<![CDATA[") {
            offset = markup[offset..]
                .find("]]>")
                .map_or(markup.len(), |found| offset + found + 3);
            continue;
        }
        if markup[offset..].starts_with("<!") || markup[offset..].starts_with("<?") {
            offset = tag_end(markup, offset);
            continue;
        }
        if offset + 1 < markup.len() && bytes[offset + 1] == b'/' {
            let end = tag_end(markup, offset);
            let close_start = skip_whitespace(bytes, offset + 2);
            let close_end = markup[close_start..end]
                .find(|character: char| character.is_ascii_whitespace() || character == '>')
                .map_or(end, |found| close_start + found);
            if markup[close_start..close_end].eq_ignore_ascii_case(name) {
                depth -= 1;
                if depth == 0 {
                    return end;
                }
            }
            offset = end;
            continue;
        }
        let tag = start_tag(markup, offset, None);
        let tag_name = &markup[tag.name_start..tag.name_end];
        if !tag.self_closing && tag_name.eq_ignore_ascii_case(name) && !is_void_element(tag_name) {
            depth += 1;
        }
        offset = tag.end;
        if !tag.self_closing && is_raw_text_element(tag_name) {
            offset = closing_tag_start(markup, offset, tag_name);
        }
    }
    markup.len()
}

fn tag_end(markup: &str, start: usize) -> usize {
    let bytes = markup.as_bytes();
    let mut offset = start;
    let mut quote = None;
    let mut escaped = false;
    while offset < markup.len() {
        let byte = bytes[offset];
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            offset += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'{' => offset = skip_braced(markup, offset).unwrap_or(markup.len()),
            b'>' => return offset + 1,
            _ => offset += 1,
        }
    }
    markup.len()
}

fn skip_braced(markup: &str, start: usize) -> Option<usize> {
    let bytes = markup.as_bytes();
    let mut offset = start;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    while offset < markup.len() {
        let byte = bytes[offset];
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            offset += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(offset + 1);
                }
            }
            _ => {}
        }
        offset += 1;
    }
    None
}

fn find_quote(markup: &str, start: usize, quote: u8) -> Option<usize> {
    let bytes = markup.as_bytes();
    let mut offset = start;
    while offset < bytes.len() {
        if bytes[offset] == quote {
            return Some(offset);
        }
        offset += 1;
    }
    None
}

fn split_top_level(source: &str, start: usize, end: usize, delimiter: u8) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut segments = Vec::new();
    let mut segment_start = start;
    let mut offset = start;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    while offset < end {
        let byte = bytes[offset];
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            offset += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ if byte == delimiter && depth == 0 => {
                segments.push((segment_start, offset));
                segment_start = offset + 1;
            }
            _ => {}
        }
        offset += 1;
    }
    segments.push((segment_start, end));
    segments
}

fn find_top_level(source: &str, start: usize, end: usize, delimiter: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut offset = start;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    while offset < end {
        let byte = bytes[offset];
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            offset += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ if byte == delimiter && depth == 0 => return Some(offset),
            _ => {}
        }
        offset += 1;
    }
    None
}

fn trim_bounds(source: &str, start: usize, end: usize) -> (usize, usize) {
    let bytes = source.as_bytes();
    let mut value_start = start;
    while value_start < end && bytes[value_start].is_ascii_whitespace() {
        value_start += 1;
    }
    let mut value_end = end;
    while value_end > value_start && bytes[value_end - 1].is_ascii_whitespace() {
        value_end -= 1;
    }
    (value_start, value_end)
}

fn skip_whitespace(bytes: &[u8], start: usize) -> usize {
    let mut offset = start;
    while offset < bytes.len() && bytes[offset].is_ascii_whitespace() {
        offset += 1;
    }
    offset
}

fn is_tag_name_end(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/' | b'{')
}

fn is_attribute_name_end(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'=' | b'>' | b'/' | b'{' | b'}' | b'<')
}

fn is_raw_text_element(name: &str) -> bool {
    ["script", "style", "pre", "textarea"]
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

fn is_void_element(name: &str) -> bool {
    [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ]
    .iter()
    .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

fn closing_tag_start(markup: &str, start: usize, name: &str) -> usize {
    let bytes = markup.as_bytes();
    let needle = format!("</{name}");
    let mut offset = start;
    while offset + needle.len() <= bytes.len() {
        if bytes[offset..offset + needle.len()].eq_ignore_ascii_case(needle.as_bytes()) {
            return offset;
        }
        offset += 1;
    }
    markup.len()
}
