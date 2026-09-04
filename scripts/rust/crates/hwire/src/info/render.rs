use std::time::Duration;

use hostkit::Route;
use serde_json::{Value, json};

use super::ColorMode;
use super::model::{Context, RouteState, Snapshot, TargetInfo, route_upper};

const DEFAULT_WIDTH: usize = 80;

pub fn compact(snapshot: &Snapshot, color: ColorMode, terminal: bool) -> String {
    let width = terminal_width().div_ceil(2).min(DEFAULT_WIDTH);
    compact_at_width(snapshot, color, terminal, width)
}

fn compact_at_width(snapshot: &Snapshot, color: ColorMode, terminal: bool, width: usize) -> String {
    let palette = Palette::new(color.enabled(terminal));
    if !snapshot.targets.is_empty() && snapshot.context == Context::Query {
        return snapshot
            .targets
            .iter()
            .map(|target| target_line(snapshot, target, &palette, terminal, width))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(session) = &snapshot.session {
        let route = session.route.map(route_upper).unwrap_or("UNKNOWN");
        let left_plain = match session.tls {
            true => format!("{route} - TLS"),
            false => route.to_string(),
        };
        let left = match (session.route, session.tls) {
            (None, _) => palette.red(&left_plain),
            (Some(_), true) => format!("{} - {}", palette.green(route), palette.tls("TLS")),
            (Some(_), false) => palette.green(route),
        };
        return endpoints(
            &left_plain,
            &left,
            session.from.name(),
            session.to.name(),
            &palette,
            terminal,
            width,
        );
    }

    let mut available: Vec<Route> = snapshot
        .routes
        .iter()
        .filter(|state| state.available)
        .map(|state| state.route)
        .collect();
    available.reverse();
    if available.is_empty() {
        return palette.red("UNREACHABLE");
    }
    available
        .into_iter()
        .map(|route| {
            let label = route_upper(route);
            if Some(route) == snapshot.preferred {
                palette.selected(label)
            } else {
                palette.dim(label)
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

pub fn verbose(snapshot: &Snapshot, color: ColorMode, terminal: bool) -> String {
    let palette = Palette::new(color.enabled(terminal));
    let mut lines = vec![format!(
        "{}  {}",
        palette.heading("hwire info"),
        palette.dim(snapshot.context.name())
    )];
    lines.push(format!("  {:<12} {}", "host", snapshot.this.name()));
    lines.push(format!("  {:<12} {}", "peer", snapshot.peer.name()));
    if let Some(session) = &snapshot.session {
        lines.push(format!(
            "  {:<12} {} --> {}",
            "connection",
            session.from.name(),
            palette.target(session.to.name())
        ));
        lines.push(format!(
            "  {:<12} {}{}",
            "transport",
            session.route.map(route_upper).unwrap_or("UNKNOWN"),
            if session.tls { " - TLS" } else { "" }
        ));
        lines.push(format!("  {:<12} {}", "evidence", session.evidence));
        if let Some(domain) = &session.domain {
            lines.push(format!("  {:<12} {}", "domain", clean(domain)));
        }
        if let Some(address) = session.client_address {
            lines.push(format!(
                "  {:<12} {}:{}",
                "client",
                address,
                session.client_port.unwrap_or_default()
            ));
        }
        if let Some(address) = session.server_address {
            lines.push(format!(
                "  {:<12} {}:{}",
                "server",
                address,
                session.server_port.unwrap_or_default()
            ));
        }
    } else {
        lines.push(format!(
            "  {:<12} {}",
            "preferred",
            snapshot.preferred.map(route_upper).unwrap_or("none")
        ));
    }

    if !snapshot.routes.is_empty() {
        lines.push(String::new());
        lines.push(palette.heading("routes"));
        for state in &snapshot.routes {
            lines.push(route_line(state, snapshot.preferred, &palette));
        }
    }
    if !snapshot.targets.is_empty() {
        lines.push(String::new());
        lines.push(palette.heading("ssh resolution"));
        for target in &snapshot.targets {
            lines.extend(target_details(target, &palette));
        }
    }
    if !snapshot.warnings.is_empty() {
        lines.push(String::new());
        for warning in &snapshot.warnings {
            lines.push(palette.yellow(&format!("! {}", clean(warning))));
        }
    }
    lines.join("\n")
}

pub fn json_document(snapshot: &Snapshot) -> Value {
    json!({
        "schema_version": 1,
        "mode": snapshot.context.name(),
        "host": snapshot.this.name(),
        "peer": snapshot.peer.name(),
        "preferred": snapshot.preferred.map(Route::name),
        "available": snapshot.routes.iter().filter(|route| route.available).map(|route| route.route.name()).collect::<Vec<_>>(),
        "session": snapshot.session.as_ref().map(|session| json!({
            "from": session.from.name(),
            "to": session.to.name(),
            "route": session.route.map(Route::name),
            "tls": session.tls,
            "client_address": session.client_address.map(|address| address.to_string()),
            "client_port": session.client_port,
            "server_address": session.server_address.map(|address| address.to_string()),
            "server_port": session.server_port,
            "domain": session.domain,
            "evidence": session.evidence,
        })),
        "routes": snapshot.routes.iter().map(|route| json!({
            "route": route.route.name(),
            "local_address": route.local.map(|address| address.to_string()),
            "peer_address": route.peer.map(|address| address.to_string()),
            "available": route.available,
            "elapsed_ms": route.elapsed.as_secs_f64() * 1_000.0,
            "error": route.error,
        })).collect::<Vec<_>>(),
        "targets": snapshot.targets.iter().map(|target| json!({
            "input": target.input,
            "hostname": target.hostname,
            "route": target.route.map(Route::name),
            "bound": target.bound,
            "proxy": target.proxy,
            "user": target.user,
            "port": target.port,
            "error": target.error,
            "master": {
                "running": target.master.running,
                "control_path": target.master.control_path,
                "age_seconds": target.master.age.map(|age| age.as_secs_f64()),
                "detail": target.master.detail,
            },
        })).collect::<Vec<_>>(),
        "warnings": snapshot.warnings,
    })
}

fn target_line(
    snapshot: &Snapshot,
    target: &TargetInfo,
    palette: &Palette,
    terminal: bool,
    width: usize,
) -> String {
    let route = target.route.map(route_upper).unwrap_or("UNKNOWN");
    let styled = if target.route.is_some() {
        palette.green(route)
    } else {
        palette.red(route)
    };
    endpoints(
        route,
        &styled,
        snapshot.this.name(),
        &target.input,
        palette,
        terminal,
        width,
    )
}

fn endpoints(
    left_plain: &str,
    left: &str,
    from: &str,
    to: &str,
    palette: &Palette,
    terminal: bool,
    width: usize,
) -> String {
    let from = clean(from);
    let to = clean(to);
    let right_plain = format!("{from} --> {to}");
    let right = format!("{from} --> {}", palette.target(&to));
    if !terminal {
        return format!("{left} {right}");
    }
    let left_width = left_plain.chars().count();
    if width <= left_width {
        return fit(left_plain, width);
    }
    let right_budget = width - left_width - 1;
    let (right_plain, right) = if right_plain.chars().count() <= right_budget {
        (right_plain, right)
    } else {
        compact_endpoint(&from, &to, palette, right_budget)
    };
    let used = left_width + right_plain.chars().count();
    let spaces = width.saturating_sub(used).max(1);
    format!("{left}{}{right}", " ".repeat(spaces))
}

fn compact_endpoint(from: &str, to: &str, palette: &Palette, budget: usize) -> (String, String) {
    let compact = format!("{from}→{to}");
    if compact.chars().count() <= budget {
        return (compact, format!("{from}→{}", palette.target(to)));
    }
    if budget <= 1 {
        return ("…".repeat(budget), "…".repeat(budget));
    }
    let hosts = budget - 1;
    let from_budget = hosts / 2;
    let to_budget = hosts - from_budget;
    let from = fit(from, from_budget);
    let to = fit(to, to_budget);
    (
        format!("{from}→{to}"),
        format!("{from}→{}", palette.target(&to)),
    )
}

fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    format!("{}…", text.chars().take(width - 1).collect::<String>())
}

fn route_line(state: &RouteState, preferred: Option<Route>, palette: &Palette) -> String {
    let status = if state.available {
        palette.green("up  ")
    } else {
        palette.red("down")
    };
    let label = format!("{:<18}", route_upper(state.route));
    let selected = if state.available && Some(state.route) == preferred {
        palette.selected(&label)
    } else {
        label
    };
    let addresses = match (state.local, state.peer) {
        (Some(local), Some(peer)) => format!("{local} --> {peer}"),
        _ => state.error.clone().unwrap_or_else(|| "unresolved".into()),
    };
    format!(
        "  {status}  {selected} {:<31} {}",
        clean(&addresses),
        duration(state.elapsed)
    )
    .trim_end()
    .to_string()
}

fn target_details(target: &TargetInfo, palette: &Palette) -> Vec<String> {
    if let Some(error) = &target.error {
        return vec![format!(
            "  {}  {}",
            palette.red(&clean(&target.input)),
            clean(error)
        )];
    }
    let mut lines = vec![format!(
        "  {}  {}  {}",
        palette.target(&clean(&target.input)),
        target.route.map(route_upper).unwrap_or("UNKNOWN"),
        clean(&target.hostname)
    )];
    if target.user.is_some() || target.port.is_some() {
        lines.push(format!(
            "    {:<10} {}{}",
            "endpoint",
            target.user.as_deref().map(clean).unwrap_or_default(),
            target
                .port
                .map(|port| format!("@{}:{port}", clean(&target.hostname)))
                .unwrap_or_default()
        ));
    }
    if let Some(bound) = &target.bound {
        lines.push(format!("    {:<10} {}", "bound", clean(bound)));
    }
    if let Some(proxy) = &target.proxy {
        lines.push(format!("    {:<10} {}", "proxy", clean(proxy)));
    }
    let age = target
        .master
        .age
        .map(duration)
        .unwrap_or_else(|| "unknown age".into());
    lines.push(format!(
        "    {:<10} {}{}",
        "master",
        if target.master.running { "up" } else { "none" },
        if target.master.running {
            format!(" ({age})")
        } else {
            String::new()
        }
    ));
    if let Some(path) = &target.master.control_path {
        lines.push(format!("    {:<10} {}", "socket", clean(path)));
    }
    if let Some(detail) = &target.master.detail {
        lines.push(format!("    {:<10} {}", "detail", clean(detail)));
    }
    lines
}

fn duration(value: Duration) -> String {
    if value.as_secs() >= 60 {
        format!("{}m {}s", value.as_secs() / 60, value.as_secs() % 60)
    } else if value.as_secs() > 0 {
        format!("{:.2}s", value.as_secs_f64())
    } else {
        format!("{:.1}ms", value.as_secs_f64() * 1_000.0)
    }
}

fn terminal_width() -> usize {
    workstation::terminal_width()
        .unwrap_or(DEFAULT_WIDTH)
        .max(1)
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

struct Palette {
    enabled: bool,
}

impl Palette {
    fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.enabled && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn selected(&self, text: &str) -> String {
        self.paint("1;38;2;52;211;153", text)
    }

    fn target(&self, text: &str) -> String {
        self.paint("1;38;2;167;139;250", text)
    }

    fn tls(&self, text: &str) -> String {
        self.paint("1;38;2;34;211;238", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("38;2;52;211;153", text)
    }

    fn red(&self, text: &str) -> String {
        self.paint("1;38;2;248;113;113", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.paint("38;2;250;204;21", text)
    }

    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    fn heading(&self, text: &str) -> String {
        self.paint("1;38;2;196;181;253", text)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/info/render_tests.rs"]
mod tests;
