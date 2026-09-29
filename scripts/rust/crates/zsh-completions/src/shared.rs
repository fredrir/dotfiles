use crate::help::{Flag, Help};
use crate::reply::{Group, Item, Reply, Tone};

// Long flags that take a value are offered as `--flag=` so the value can follow directly.
pub fn flags(help: &Help, reply: &mut Reply) {
    let mut plain = Vec::new();
    let mut valued = Vec::new();
    for flag in &help.flags {
        for name in &flag.names {
            if flag.value.is_some() && name.starts_with("--") {
                valued.push(Item::new(format!("{name}="), &flag.description));
            } else {
                plain.push(Item::new(name, &flag.description));
            }
        }
    }
    reply.group(
        Group::new("options", "option")
            .tone(Tone::Info)
            .items(plain),
    );
    reply.group(
        Group::new("options", "option")
            .tone(Tone::Info)
            .suffix("", false)
            .items(valued),
    );
}

// What the help text itself says about a value: its choices, or that it names a path.
pub fn generic_value(flag: &Flag, reply: &mut Reply) {
    let Some(value) = &flag.value else { return };
    if !value.choices.is_empty() {
        reply.group(
            Group::new("values", &value.placeholder)
                .unsorted()
                .items(value.choices.iter().map(Item::bare)),
        );
    } else if value.names_a_directory() {
        reply.directories();
    } else if value.names_a_path() {
        reply.files();
    } else {
        reply.message(format!(
            "{} for {}",
            value.placeholder,
            flag.names.join(", ")
        ));
    }
}

// The element being typed in a comma-separated list, and the elements before it.
pub fn list_element<'a>(prefix: &'a str, reply: &mut Reply) -> (&'a str, Vec<&'a str>) {
    match prefix.rfind(',') {
        Some(at) => {
            reply.skip(&prefix[..=at]);
            (&prefix[at + 1..], prefix[..at].split(',').collect())
        }
        None => (prefix, Vec::new()),
    }
}
