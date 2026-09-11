use super::Result;
pub fn between(text: &str, name: &str, body: &[String]) -> Result<String> {
    let start = format!("theme:{name}");
    let end = format!("{start}:end");
    let lines = text.split('\n').collect::<Vec<_>>();
    let first = lines
        .iter()
        .position(|line| line.contains(&start) && !line.contains(&end))
        .ok_or_else(|| format!("marker '{start}' not found"))?;
    let last = lines
        .iter()
        .position(|line| line.contains(&end))
        .filter(|last| *last > first)
        .ok_or_else(|| format!("marker '{end}' not found"))?;
    let indent = &lines[first][..lines[first].len() - lines[first].trim_start().len()];
    let mut result = lines[..=first]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    result.extend(body.iter().map(|s| {
        if s.is_empty() {
            String::new()
        } else {
            format!("{indent}{s}")
        }
    }));
    result.extend(lines[last..].iter().map(|s| s.to_string()));
    Ok(result.join("\n"))
}
fn bounds(lines: &[String], header: &str) -> Result<(usize, usize)> {
    let marker = format!("[{header}]");
    let start = lines
        .iter()
        .position(|l| l == &marker)
        .ok_or_else(|| format!("section '{marker}' not found"))?;
    let end = (start + 1..lines.len())
        .find(|i| lines[*i].starts_with('['))
        .unwrap_or(lines.len());
    Ok((start, end))
}
pub fn section(text: &str, header: &str, body: &[String]) -> Result<String> {
    let mut lines = text.split('\n').map(str::to_string).collect::<Vec<_>>();
    let (start, end) = bounds(&lines, header)?;
    let blanks = lines[start + 1..end]
        .iter()
        .rev()
        .take_while(|l| l.is_empty())
        .count();
    let replacement = body
        .iter()
        .cloned()
        .chain(std::iter::repeat_n(String::new(), blanks));
    lines.splice(start + 1..end, replacement);
    Ok(lines.join("\n"))
}
pub fn ini(text: &str, header: &str, key: &str, value: &str) -> Result<String> {
    let mut lines = text.split('\n').map(str::to_string).collect::<Vec<_>>();
    let (start, end) = bounds(&lines, header)?;
    if let Some(index) = (start + 1..end).find(|i| lines[*i].split('=').next() == Some(key)) {
        lines[index] = format!("{key}={value}");
    } else {
        let index = (start + 1..end)
            .find(|i| {
                let name = lines[*i].split('=').next().unwrap_or("");
                !name.is_empty() && name > key
            })
            .unwrap_or(end);
        lines.insert(index, format!("{key}={value}"));
    }
    Ok(lines.join("\n"))
}
pub fn lua(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
pub fn lua_key(name: &str) -> String {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid&&!"and break do else elseif end false for function goto if in local nil not or repeat return then true until while".split_whitespace().any(|k|k==name){name.into()}else{format!("[{}]",lua(name))}
}
pub fn lines(lines: Vec<String>) -> String {
    lines.join("\n") + "\n"
}
