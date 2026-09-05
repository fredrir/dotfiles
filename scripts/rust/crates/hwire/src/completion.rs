use clap::{Arg, Command};

const MEASURE: [&str; 16] = [
    "route", "all", "both", "time", "streams", "samples", "latency", "up", "down", "at", "json",
    "color", "info", "shell", "help", "version",
];

const INFO: [&str; 7] = [
    "info", "verbose", "watch", "json", "color", "help", "version",
];

const WATCH: [&str; 2] = ["interval", "notify"];

const AT: [&str; 1] = ["token"];

const HOSTS: &str = "      '*:SSH host:_hosts'\n";

const HEADER: &str = r#"
# clap cannot express "offer this option only after --info/--watch". Keep its
# generated function for subcommands and layer a state-aware root completer on
# top so invalid hwire modes are not suggested.
functions[_hwire_clap]=$functions[_hwire]

_hwire() {
  local word info_mode=0
  for word in "${words[@]}"; do
    if [[ $word == --info || ($word == -*i* && $word != --*) ]]; then
      info_mode=1
      break
    fi
  done

  if [[ ${words[2]-} == ("#;

const DISPATCH: &str = r#") ]]; then
    _hwire_clap "$@"
    return
  fi

  if (( info_mode )); then
    local -a info_arguments
    info_arguments=(
"#;

const WATCH_HEAD: &str = r#"    )
    if (( ${words[(I)--watch]} )); then
      info_arguments+=(
"#;

const WATCH_TAIL: &str = r#"      )
    fi
    _arguments -s -S $info_arguments
    return
  fi

  local -a measure_arguments
  measure_arguments=(
"#;

const AT_HEAD: &str = r#"  )
  if (( ${words[(I)--at]} )); then
    measure_arguments+=(
"#;

const AT_TAIL: &str = r#"    )
  fi
  _arguments -s -S $measure_arguments
}

compdef _hwire hwire
"#;

pub fn zsh(command: &Command) -> String {
    let subcommands = subcommands(command);
    let info_mode = info_mode();
    let measure_mode = measure_mode();
    [
        HEADER,
        &subcommands.join("|"),
        DISPATCH,
        &specs(command, &INFO, &info_mode, "      "),
        HOSTS,
        WATCH_HEAD,
        &specs(command, &WATCH, &info_mode, "        "),
        WATCH_TAIL,
        &specs(command, &MEASURE, &measure_mode, "    "),
        &format!("    '1:command:({})'\n", subcommands.join(" ")),
        AT_HEAD,
        &specs(command, &AT, &measure_mode, "      "),
        AT_TAIL,
    ]
    .concat()
}

fn info_mode() -> Vec<&'static str> {
    [INFO.as_slice(), WATCH.as_slice()].concat()
}

fn measure_mode() -> Vec<&'static str> {
    [MEASURE.as_slice(), AT.as_slice()].concat()
}

fn subcommands(command: &Command) -> Vec<String> {
    command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .map(|child| child.get_name().to_string())
        .collect()
}

fn specs(command: &Command, ids: &[&str], mode: &[&str], indent: &str) -> String {
    ids.iter()
        .filter_map(|id| argument(command, id))
        .map(|argument| format!("{indent}{}\n", spec(command, argument, mode)))
        .collect()
}

fn argument<'a>(command: &'a Command, id: &str) -> Option<&'a Arg> {
    command
        .get_arguments()
        .find(|argument| argument.get_id() == id && !argument.is_hide_set())
}

fn spec(command: &Command, argument: &Arg, mode: &[&str]) -> String {
    let excluded = exclusions(command, argument, mode);
    let group = match excluded.is_empty() {
        true => String::new(),
        false => format!("({excluded})"),
    };
    let tail = tail(argument);
    match (argument.get_short(), argument.get_long()) {
        (Some(short), Some(long)) if group.is_empty() => format!("{{-{short},--{long}}}'{tail}'"),
        (Some(short), Some(long)) => format!("'{group}'{{-{short},--{long}}}'{tail}'"),
        (short, long) => format!("'{group}{}{tail}'", spelling(short, long)),
    }
}

fn spelling(short: Option<char>, long: Option<&str>) -> String {
    match (short, long) {
        (_, Some(long)) => format!("--{long}"),
        (Some(short), None) => format!("-{short}"),
        (None, None) => String::new(),
    }
}

fn tail(argument: &Arg) -> String {
    let description = description(argument)
        .map(|help| format!("[{help}]"))
        .unwrap_or_default();
    if !argument.get_action().takes_values() {
        return description;
    }
    format!("={description}:{}:{}", message(argument), choices(argument))
}

fn description(argument: &Arg) -> Option<String> {
    argument
        .get_help()
        .map(|help| escape(&help.to_string(), &['\\', '[', ']']))
}

fn message(argument: &Arg) -> String {
    let metavar = argument
        .get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.to_string())
        .unwrap_or_else(|| argument.get_id().to_string());
    escape(&metavar.to_lowercase(), &['\\', ':'])
}

fn choices(argument: &Arg) -> String {
    let values: Vec<String> = argument
        .get_possible_values()
        .into_iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_string())
        .collect();
    match values.is_empty() {
        true => String::new(),
        false => format!("({})", values.join(" ")),
    }
}

fn exclusions(command: &Command, argument: &Arg, mode: &[&str]) -> String {
    let conflicts: Vec<&Arg> = command
        .get_arguments()
        .filter(|other| {
            other.get_id() != argument.get_id()
                && !other.is_hide_set()
                && mode.iter().any(|id| other.get_id() == id)
                && conflicting(command, argument, other)
        })
        .collect();
    let paired = argument.get_short().is_some() && argument.get_long().is_some();
    if conflicts.is_empty() && !paired {
        return String::new();
    }
    std::iter::once(argument)
        .chain(conflicts)
        .map(names)
        .collect::<Vec<_>>()
        .join(" ")
}

fn names(argument: &Arg) -> String {
    [
        argument.get_short().map(|short| format!("-{short}")),
        argument.get_long().map(|long| format!("--{long}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

fn conflicting(command: &Command, one: &Arg, other: &Arg) -> bool {
    declared(command, one, other) || declared(command, other, one)
}

fn declared(command: &Command, argument: &Arg, other: &Arg) -> bool {
    command
        .get_arg_conflicts_with(argument)
        .iter()
        .any(|found| found.get_id() == other.get_id())
}

fn escape(text: &str, specials: &[char]) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\'' => escaped.push_str("'\\''"),
            _ if specials.contains(&character) => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
#[path = "../tests/unit/completion_tests.rs"]
mod tests;
