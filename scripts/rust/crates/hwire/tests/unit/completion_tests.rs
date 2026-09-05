use clap::CommandFactory;

use super::*;
use crate::Cli;

fn built() -> Command {
    let mut command = Cli::command();
    command.build();
    command
}

fn grouped() -> Vec<&'static str> {
    MEASURE
        .iter()
        .chain(INFO.iter())
        .chain(WATCH.iter())
        .chain(AT.iter())
        .copied()
        .collect()
}

fn descriptions(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('\'') || line.starts_with('{'))
        .filter_map(|line| {
            let start = line.find('[')?;
            let end = line.rfind(']')?;
            Some(line[start + 1..end].to_string())
        })
        .collect()
}

#[test]
fn every_description_in_the_overlay_is_a_clap_help_string() {
    let command = built();
    let helps: Vec<String> = command.get_arguments().filter_map(description).collect();
    let found = descriptions(&zsh(&command));
    assert!(!found.is_empty());
    for description in found {
        assert!(
            helps.contains(&description),
            "{description:?} is not from clap"
        );
    }
}

#[test]
fn every_grouped_argument_carries_its_own_clap_help() {
    let command = built();
    let text = zsh(&command);
    for id in grouped() {
        let argument = argument(&command, id).unwrap_or_else(|| panic!("{id} is an argument"));
        let Some(help) = description(argument) else {
            continue;
        };
        assert!(text.contains(&format!("[{help}]")), "{id} lost its help");
    }
}

#[test]
fn value_options_carry_the_clap_metavar_and_possible_values() {
    let command = built();
    let text = zsh(&command);
    let route = argument(&command, "route").expect("--route is an argument");
    let values: Vec<String> = route
        .get_possible_values()
        .into_iter()
        .map(|value| value.get_name().to_string())
        .collect();
    assert!(
        text.contains(&format!(":route:({})", values.join(" "))),
        "{text}"
    );
    assert!(text.contains(":seconds:"), "{text}");
}
