#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Indent,
    Program,
    Directory,
    Name,
    Text,
    Json,
    File,
    JsonFile,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Flag {
    pub short: Option<char>,
    pub long: &'static str,
    pub values: &'static [Value],
    pub description: &'static str,
    // The probe passes it on, since it changes the input or binds variables.
    pub probe: bool,
    pub repeatable: bool,
    pub excludes: &'static [&'static str],
    // Only meaningful on its own, such as `--version`.
    pub alone: bool,
}

const fn flag(short: Option<char>, long: &'static str, description: &'static str) -> Flag {
    Flag {
        short,
        long,
        values: &[],
        description,
        probe: false,
        repeatable: false,
        excludes: &[],
        alone: false,
    }
}

impl Flag {
    const fn takes(mut self, values: &'static [Value]) -> Flag {
        self.values = values;
        self
    }

    const fn probed(mut self) -> Flag {
        self.probe = true;
        self
    }

    const fn repeatable(mut self) -> Flag {
        self.repeatable = true;
        self
    }

    const fn excludes(mut self, excludes: &'static [&'static str]) -> Flag {
        self.excludes = excludes;
        self
    }

    const fn alone(mut self) -> Flag {
        self.alone = true;
        self
    }

    pub fn names(&self) -> impl Iterator<Item = String> {
        self.short
            .map(|short| format!("-{short}"))
            .into_iter()
            .chain(std::iter::once(format!("--{}", self.long)))
    }
}

pub const FLAGS: &[Flag] = &[
    flag(Some('n'), "null-input", "use null as the single input").probed(),
    flag(Some('R'), "raw-input", "read each line as a string").probed(),
    flag(Some('s'), "slurp", "read all inputs into one array").probed(),
    flag(Some('c'), "compact-output", "compact output").excludes(&["tab", "indent"]),
    flag(Some('r'), "raw-output", "print strings without quotes")
        .excludes(&["join-output", "raw-output0"]),
    flag(None, "raw-output0", "like -r, with NUL after each output")
        .excludes(&["raw-output", "join-output"]),
    flag(Some('j'), "join-output", "like -r, without newlines")
        .excludes(&["raw-output", "raw-output0"]),
    flag(Some('a'), "ascii-output", "escape non-ASCII characters"),
    flag(Some('S'), "sort-keys", "sort object keys"),
    flag(Some('C'), "color-output", "colorize output").excludes(&["monochrome-output"]),
    flag(Some('M'), "monochrome-output", "disable color").excludes(&["color-output"]),
    flag(None, "tab", "indent with tabs").excludes(&["indent", "compact-output"]),
    flag(None, "indent", "indent with n spaces")
        .takes(&[Value::Indent])
        .excludes(&["tab", "compact-output"]),
    flag(None, "unbuffered", "flush after each output"),
    flag(None, "stream", "read input as [path, leaf] events")
        .probed()
        .excludes(&["stream-errors"]),
    flag(
        None,
        "stream-errors",
        "like --stream, with parse errors as arrays",
    )
    .probed()
    .excludes(&["stream"]),
    flag(None, "seq", "application/json-seq input and output").probed(),
    flag(Some('f'), "from-file", "read the filter from a file").takes(&[Value::Program]),
    flag(Some('L'), "library-path", "search modules in a directory")
        .takes(&[Value::Directory])
        .probed()
        .repeatable(),
    flag(None, "arg", "set $name to a string")
        .takes(&[Value::Name, Value::Text])
        .probed()
        .repeatable(),
    flag(None, "argjson", "set $name to a JSON value")
        .takes(&[Value::Name, Value::Json])
        .probed()
        .repeatable(),
    flag(None, "slurpfile", "set $name to the JSON values in a file")
        .takes(&[Value::Name, Value::JsonFile])
        .probed()
        .repeatable(),
    flag(None, "rawfile", "set $name to the contents of a file")
        .takes(&[Value::Name, Value::File])
        .probed()
        .repeatable(),
    flag(None, "args", "remaining arguments are strings").excludes(&["jsonargs"]),
    flag(None, "jsonargs", "remaining arguments are JSON values").excludes(&["args"]),
    flag(Some('e'), "exit-status", "exit status from the last output"),
    flag(Some('V'), "version", "show the version").alone(),
    flag(None, "build-configuration", "show the build configuration").alone(),
    flag(Some('h'), "help", "show help").alone(),
];

pub fn long(name: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|flag| flag.long == name)
}

pub fn short(name: char) -> Option<&'static Flag> {
    FLAGS.iter().find(|flag| flag.short == Some(name))
}
