use hostkit::Host;
use workstation::Style;

use crate::domain;
use crate::probe::Answer;

const ADDRESS: usize = 19;

pub fn list(style: &Style, peer: Host, answers: &[Answer]) -> String {
    answers
        .iter()
        .map(|answer| {
            let (state, domain) = match answer.up {
                true => (style.green("up  "), domain::name(peer, answer.route)),
                false => (style.red("down"), String::new()),
            };
            let address = answer
                .peer_socket()
                .map(|address| address.to_string())
                .unwrap_or_else(|| "unresolved".into());
            let line = format!(
                "{state}  {:<10} {:<ADDRESS$} {domain}",
                answer.route.name(),
                address
            );
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "../tests/unit/render_tests.rs"]
mod tests;
