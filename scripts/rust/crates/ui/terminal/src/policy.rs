#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiPolicy {
    pub interactive: bool,
    pub color: bool,
    pub motion: bool,
}

pub fn environment_flag_enabled(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

pub fn capable(input: bool, output: bool, term: Option<&str>, ci: Option<&str>) -> bool {
    input
        && output
        && term.is_none_or(|value| !value.eq_ignore_ascii_case("dumb"))
        && !ci.is_some_and(environment_flag_enabled)
}

pub fn reduced_motion_requested(tool_env: &str) -> bool {
    [
        tool_env,
        "PREFERS_REDUCED_MOTION",
        "REDUCE_MOTION",
        "REDUCED_MOTION",
    ]
    .into_iter()
    .any(|name| {
        std::env::var(name)
            .ok()
            .is_some_and(|value| environment_flag_enabled(&value))
    })
}

impl UiPolicy {
    pub fn from_signals(
        input: bool,
        output: bool,
        term: Option<&str>,
        ci: Option<&str>,
        no_color: bool,
        clicolor: Option<&str>,
        reduced_motion: bool,
    ) -> Self {
        let interactive = capable(input, output, term, ci);
        let color = interactive && !no_color && clicolor != Some("0");
        Self {
            interactive,
            color,
            motion: color && !reduced_motion,
        }
    }
    pub fn detect(input: bool, output: bool, tool_motion_env: &str) -> Self {
        Self::from_signals(
            input,
            output,
            std::env::var("TERM").ok().as_deref(),
            std::env::var("CI").ok().as_deref(),
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("CLICOLOR").ok().as_deref(),
            reduced_motion_requested(tool_motion_env),
        )
    }
}
