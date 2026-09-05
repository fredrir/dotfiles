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

fn exclusion_names(line: &str) -> Vec<String> {
    let Some(rest) = line.trim().strip_prefix("'(") else {
        return Vec::new();
    };
    let Some((group, _)) = rest.split_once(')') else {
        return Vec::new();
    };
    group.split_whitespace().map(str::to_string).collect()
}

fn spellings(command: &Command, ids: &[&str]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| argument(command, id))
        .flat_map(|argument| {
            [
                argument.get_short().map(|short| format!("-{short}")),
                argument.get_long().map(|long| format!("--{long}")),
            ]
        })
        .flatten()
        .collect()
}

#[test]
fn no_exclusion_names_an_argument_outside_its_mode_array() {
    let command = built();
    for (ids, mode) in [
        (INFO.as_slice(), info_mode()),
        (WATCH.as_slice(), info_mode()),
        (MEASURE.as_slice(), measure_mode()),
        (AT.as_slice(), measure_mode()),
    ] {
        let allowed = spellings(&command, &mode);
        let rendered = specs(&command, ids, &mode, "");
        for line in rendered.lines() {
            for name in exclusion_names(line) {
                assert!(allowed.contains(&name), "{name} is outside its mode array");
            }
        }
    }
}

#[test]
fn the_info_spec_drops_the_measure_flags_the_info_array_never_offers() {
    let command = built();
    let rendered = specs(&command, &INFO, &info_mode(), "");
    let info = rendered
        .lines()
        .find(|line| line.contains("--info"))
        .expect("--info has a spec");
    assert_eq!(exclusion_names(info), ["-i", "--info"]);
    assert!(
        specs(&command, &MEASURE, &measure_mode(), "")
            .lines()
            .any(|line| exclusion_names(line).contains(&"--token".to_string())),
        "{rendered}"
    );
}
