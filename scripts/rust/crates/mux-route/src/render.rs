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
            let line = format!(
                "{state}  {:<10} {:<ADDRESS$} {domain}",
                answer.route.name(),
                answer.peer.to_string()
            );
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hostkit::{MUX_PORT, Route};
    use std::net::SocketAddrV4;

    fn answer(route: Route, up: bool) -> Answer {
        Answer {
            route,
            peer: SocketAddrV4::new(Host::Archie.address(route).unwrap(), MUX_PORT),
            up,
        }
    }

    #[test]
    fn a_route_that_answered_carries_the_domain_to_attach() {
        let printed = list(
            &Style::plain(),
            Host::Archie,
            &[answer(Route::Cable, true), answer(Route::Wifi, false)],
        );
        let lines: Vec<&str> = printed.lines().collect();
        assert!(lines[0].starts_with("up    cable"), "{printed}");
        assert!(lines[0].ends_with("archie-cable"), "{printed}");
        assert!(lines[1].starts_with("down  wifi"), "{printed}");
        assert!(!lines[1].contains("archie"), "{printed}");
        assert!(!lines[1].ends_with(' '), "{printed}");
    }
}
