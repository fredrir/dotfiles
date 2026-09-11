pub use ui_terminal::text::{truncate_back, truncate_front};

pub fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", plural(count, one, many))
}

#[cfg(test)]
#[path = "../tests/unit/text_tests.rs"]
mod tests;
