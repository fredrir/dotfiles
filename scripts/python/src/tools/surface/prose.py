"""How each command and flag is described in `docs/cli`.

The tables in those pages are generated, but their wording is not: help text is
imperative and terse because it is read beside the command being typed, while a
reference page reads better in the third person and can say what the source of
a value is. Keeping the sentences here rather than in the parsers preserves the
voice those pages were written in -- and `tests/surface/test_prose.py` fails
when a command or flag arrives, leaves, or is renamed without this file
following, which is the drift the tables used to accumulate silently.

Commands are keyed by the command as it is typed. Flags are keyed by page and
long spelling, because `--all` means one thing to `gdd` and another to
`sysinfo bench list`, and one row has to cover both wherever a page documents
several tools.
"""

# Described once for every tool, since a flag that behaves the same everywhere
# should not be explained eleven times.
STANDARD = {
    "--help": "Shows help for the selected command and exits.",
    "--completions": "Prints a shell completion script for the named shell and exits.",
    "--version": "Prints the version and exits.",
}

COMMANDS = {
    "acp": "Copies Archie's text clipboard to this machine.",
    "cpa": "Copies the local text clipboard to Archie.",
    "cpas": "Copies the local text clipboard to Archie, keeping it out of clipboard history.",
    "count": "Counts items inside a directory.",
    "dotfile": "Manages this repository's symlinks, packages, themes, secrets, and system files.",
    "dotfile add": "Moves a live config into the repository and symlinks it back.",
    "dotfile remove": "Moves a tracked path out of the repository and keeps it live.",
    "dotfile docs": "Regenerates the command tables in `docs/cli` from the tools themselves.",
    "dotfile packages": "Regenerates `config/packages.dotfile` and `PACKAGES.md`.",
    "dotfile sync": "Reconciles `$HOME` with a profile by linking, merging, and applying secrets.",
    "dotfile status": "Shows link state for every file in a profile.",
    "dotfile check": "Checks a profile's links, required tools, and packages.",
    "dotfile secret": "Keeps private material out of the repository.",
    "dotfile secret scan": "Scans for leaked tokens, private values, and encryption invariants.",
    "dotfile secret init": "Creates this machine's age identity and prints its public key.",
    "dotfile secret enroll": "Adds a recipient or enrolls this machine when no key is provided.",
    "dotfile secret revoke": "Removes a recipient and gives every encrypted file a new data key.",
    "dotfile secret roll": "Replaces a recipient's key while keeping its label.",
    "dotfile secret rekey": "Gives every encrypted file a new data key without changing recipients.",
    "dotfile secret keys": "Lists the enrolled recipients.",
    "dotfile secret sync": "Regenerates `.sops.yaml` from `config/keys.dotfile`.",
    "dotfile secret doctor": "Checks identities, recipients, hooks, and encrypted files.",
    "dotfile secret add": "Encrypts a live file into the repository and keeps it in place.",
    "dotfile secret edit": "Opens a tracked secret in `$EDITOR` and reapplies it.",
    "dotfile secret apply": "Decrypts every tracked secret to its destination.",
    "dotfile secret status": "Shows what each tracked secret looks like on this machine.",
    "dotfile secret vars": "Lists the names that secret templates can reference.",
    "dotfile secret clean": "Removes materialized secrets from their destinations.",
    "dotfile system": "Tracks root-owned files under `/etc` and installs them as root.",
    "dotfile system status": "Compares tracked system files with their installed versions.",
    "dotfile system diff": "Shows what would change on disk without modifying anything.",
    "dotfile system install": "Installs tracked system files at their destinations as root.",
    "dotfile system add": "Copies a root-owned file into the repository.",
    "dotfile theme": "Stamps selected theme profiles into generated configuration files.",
    "dotfile theme apply": "Regenerates every config from the selected theme profiles.",
    "dotfile theme check": "Reports what theme generation would change without writing.",
    "dotfile theme status": (
        "Shows each group's resolved profile and whether generated files have drifted."
    ),
    "dotfile theme show": "Previews a profile's palette, roles, fonts, and terminal colors.",
    "dotfile theme switch": "Assigns a profile globally, to a group, or to a package.",
    "dotfile theme outputs": "Prints the files owned by the theme generator.",
    "dotfile-format": "Formats a tree by handing each language to the tool that owns it.",
    "dotfmt": "Formats the `.conf`, `.config`, and `.dotfile` files in a tree.",
    "flatten": "Lifts a directory's contents out of the directories holding them.",
    "gdd": (
        "Discards tracked and untracked working-tree changes while preserving ignored "
        "files and nested repositories."
    ),
    "gget": ("Downloads a file or directory from a GitHub repository into the current directory."),
    "gpp": "Stages everything, commits with the supplied message, and pushes the commit.",
    "hwire": "Measures latency and throughput between two machines.",
    "hwire serve": "Answers measurements until told to stop.",
    "path": "Prints the repository-relative or home-relative path of a target.",
    "size": "Reports sizes and line counts for files and directories.",
    "sysinfo": "Summarizes the current machine's environment and hardware.",
    "sysinfo bench": "Opens the benchmark command menu or prints its help.",
    "sysinfo bench run": "Measures the current machine and optionally stores the result.",
    "sysinfo bench show": "Displays a stored benchmark run.",
    "sysinfo bench list": "Lists stored benchmark runs.",
    "sysinfo bench health": "Reports warnings derived from benchmark history.",
    "sysinfo bench compare": "Compares two benchmark runs.",
    "sysinfo bench trend": "Shows one benchmark metric over time.",
    "sysinfo bench baseline": (
        "Sets, clears, or shows the baseline run for a machine and hardware configuration."
    ),
    "sysinfo bench document": "Regenerates the benchmark documentation from stored runs.",
    "sysinfo bench prune": (
        "Removes superseded runs while preserving baselines and configuration history."
    ),
    "tardirs": "Shows the directory tree of a tar archive with an entry count for each directory.",
    "transcript": "Archives AI agent sessions as Obsidian notes.",
    "transcript capture": "Wraps clipboard text as a transcript note in the vault.",
    "transcript import": "Imports a Claude Code or Codex session as a transcript note.",
    "transcript list": "Lists recent Claude Code and Codex sessions.",
    "transcript add": "Tracks a project for transcript sync.",
    "transcript rm": "Stops tracking a project while preserving its existing notes.",
    "transcript migrate": "Moves existing transcript groups to their configured destinations.",
    "transcript sync": "Syncs allowlisted Claude Code and Codex sessions into the vault.",
}

FLAGS = {
    ("clipboard", "--sensitive"): "Keeps the copy out of Archie's clipboard history.",
    ("count", "--recursive"): (
        "Counts every entry below the directory instead of only its direct children."
    ),
    ("count", "--no-hidden"): "Excludes hidden entries and everything inside hidden directories.",
    ("dotfile", "--shared"): "Places an added file in the shared package group.",
    ("dotfile", "--linux"): "Places an added file in the `linux/common` package group.",
    ("dotfile", "--arch"): "Places an added file in the `linux/arch` package group.",
    ("dotfile", "--ubuntu"): "Places an added file in the `linux/ubuntu` package group.",
    ("dotfile", "--kde"): "Places an added file in the `linux/kde` package group.",
    ("dotfile", "--hyprland"): "Places an added file in the `linux/hyprland` package group.",
    ("dotfile", "--server"): "Places an added config in the `linux/server` package group.",
    ("dotfile", "--macos"): "Places an added file in the `macos` package group.",
    ("dotfile", "--pkg"): "Selects the package name when adding a config, secret, or system file.",
    ("dotfile", "--description"): "Adds a package description to `PACKAGES.md`.",
    ("dotfile", "--check"): "Reports documentation drift instead of writing the tables.",
    ("dotfile", "--dry-run"): "Reports actions without changing files.",
    ("dotfile", "--override"): "Selects a machine override with `<group>=<name|none>`.",
    ("dotfile", "--force"): (
        "Forces repository resolution during sync or overwrites locally edited secret destinations."
    ),
    ("dotfile", "--resolve"): (
        "Selects `skip`, `repo`, or `live` resolution for locally changed configs."
    ),
    ("dotfile", "--push"): "Pushes changes, then pulls and syncs the other machine.",
    ("dotfile", "--to"): "Selects the machine targeted by `--push`.",
    ("dotfile", "--all"): "Shows every finding or file location instead of summarized output.",
    ("dotfile", "--staged"): "Scans the content staged for commit.",
    ("dotfile", "--commits"): "Scans blobs added within a revision-list range.",
    ("dotfile", "--no-canaries"): "Skips the private-value tier of secret scanning.",
    ("dotfile", "--using"): (
        "Uses the selected identity file for recipient and re-encryption operations."
    ),
    ("dotfile", "--rewrap"): "Updates the recipients on every encrypted file during secret sync.",
    ("dotfile", "--marker"): "Forces the `.secret` package marker on or off.",
    ("dotfile", "--unused"): "Lists only variable names that no secret template references.",
    ("dotfile", "--yes"): "Installs system files without asking for confirmation.",
    ("dotfile", "--group"): "Selects the package group for an added system file.",
    ("dotfile", "--stageable"): "Prints only generated theme files that are safe to stage.",
    ("dotfile-format", "--check"): (
        "Verifies formatting and runs each language's linter instead of writing anything."
    ),
    ("dotfile-format", "--add"): (
        "Offers this repository's tool configuration to the target, asking per file."
    ),
    ("dotfile-format", "--sync"): (
        "Replaces the tool configuration the target already has, without asking."
    ),
    ("dotfile-format", "--verbose"): "Names every file as it is formatted.",
    ("dotfile-format", "--quiet"): "Reports nothing but failures.",
    ("dotfmt", "--check"): "Reports files that are not formatted instead of rewriting them.",
    ("dotfmt", "--stdin"): (
        "Formats standard input as the named file and writes the result to standard output."
    ),
    ("dotfmt", "--owns"): (
        "Reads NUL-separated paths from standard input and answers with the ones it formats."
    ),
    ("dotfmt", "--verbose"): "Names every file as it is formatted.",
    ("dotfmt", "--quiet"): "Reports nothing but failures.",
    ("flatten", "--deep"): (
        "Brings every nested entry to the top instead of removing only wrappers."
    ),
    ("flatten", "--dry-run"): "Shows what would happen without changing the directory.",
    ("flatten", "--yes"): "Runs without asking for confirmation.",
    ("flatten", "--verbose"): "Names every move as it is made.",
    ("flatten", "--all"): "Lists every row instead of truncating sections after 12 rows.",
    ("git", "--dry-run"): "Shows what `gdd` would discard without changing the working tree.",
    ("git", "--all"): "Makes `gdd` list every entry instead of truncating its sections.",
    ("git", "--yes"): "Skips confirmation before discarding changes or replacing a download.",
    ("git", "--fredrir"): "Reads the `gget` target as a repository owned by `fredrir`.",
    ("git", "--branch"): "Selects the branch or tag from which `gget` downloads.",
    ("hwire", "--route"): "Selects the cable, Wi-Fi, LAN, or Tailscale route to measure.",
    ("hwire", "--all"): "Measures every available route sequentially.",
    ("hwire", "--both"): "Provides a compatibility spelling for `--all`.",
    ("hwire", "--time"): "Sets the transfer duration for each direction.",
    ("hwire", "--streams"): "Sets the number of concurrent transfer connections.",
    ("hwire", "--samples"): "Limits the number of round trips timed.",
    ("hwire", "--latency"): "Measures round-trip latency without running transfers.",
    ("hwire", "--up"): "Transfers only from this machine to the peer.",
    ("hwire", "--down"): "Transfers only from the peer to this machine.",
    ("hwire", "--at"): "Measures an already-running server without starting one over SSH.",
    ("hwire", "--token"): "Uses or requires the server's authentication token.",
    ("hwire", "--json"): "Prints the measurement as JSON.",
    ("hwire", "--bind"): "Sets the address on which `hwire serve` listens.",
    ("hwire", "--port"): "Sets the server port, with zero selecting an available port.",
    ("hwire", "--idle"): "Sets how long an idle server waits before exiting.",
    ("path", "--full"): "Prints the full path instead of a relative one.",
    ("size", "-r"): "Lists the immediate contents of the directory.",
    ("size", "-R"): "Lists the contents of the directory recursively.",
    ("size", "--lines"): "Counts lines instead of bytes.",
    ("size", "--apparent"): "Measures logical lengths rather than the space actually taken up.",
    ("size", "--limit"): "Limits how deep the recursive listing goes.",
    ("size", "--all"): "Includes hidden entries in listings, which totals count either way.",
    ("size", "--ignore"): "Leaves matching entries out of both the listing and the totals.",
    ("size", "--one-file-system"): (
        "Stays on the filesystem the target sits on, leaving other mounts out."
    ),
    ("sysinfo", "--pretty"): "Shows the complete branded hardware presentation.",
    ("sysinfo", "--full"): "Includes the extended hardware inventory.",
    ("sysinfo", "--health"): "Explains active errors and warnings.",
    ("sysinfo", "--tier"): "Selects the quick, standard, or heavy benchmark tier.",
    ("sysinfo", "--only"): "Limits a run to a comma-separated list of measurement families.",
    ("sysinfo", "--note"): "Records why a benchmark run was taken.",
    ("sysinfo", "--tag"): "Adds a label to a benchmark run.",
    ("sysinfo", "--host"): "Selects the host to record, list, assess, or prune.",
    ("sysinfo", "--workdir"): "Selects the directory used by the disk benchmark tier.",
    ("sysinfo", "--force"): "Runs benchmarks despite unsuitable measurement conditions.",
    ("sysinfo", "--no-save"): "Prints a benchmark result without storing it.",
    ("sysinfo", "--baseline"): "Pins the new benchmark run as its machine's baseline.",
    ("sysinfo", "--json"): "Emits a run or comparison as JSON.",
    ("sysinfo", "--limit"): "Limits the number of stored runs listed.",
    ("sysinfo", "--all"): "Includes noisy and aborted runs in a listing.",
    ("sysinfo", "--keep"): "Sets the number of runs retained per machine configuration.",
    ("sysinfo", "--dry-run"): "Reports which runs would be pruned without deleting them.",
    ("sysinfo", "--yes"): "Prunes stored runs without asking for confirmation.",
    ("transcript", "--provider"): "Overrides clipboard provider detection for `capture`.",
    ("transcript", "--raw"): "Skips secret redaction during capture, import, or sync.",
    ("transcript", "--quiet"): "Prints nothing after a successful capture or sync.",
    ("transcript", "--fallback"): "Selects a snapshot file when the clipboard is empty.",
    ("transcript", "--latest"): "Imports the newest available session.",
    ("transcript", "--limit"): (
        "Sets how many sessions appear in the import picker or session list."
    ),
    ("transcript", "--tools"): "Includes tool calls in imported or synchronized notes.",
    ("transcript", "--name"): (
        "Sets the tracked project name instead of deriving it from the directory."
    ),
    ("transcript", "--group"): "Assigns a tracked project to a transcript group.",
    ("transcript", "--verbose"): "Lists every file in the migration preview.",
    ("transcript", "--dry-run"): "Reports sync changes without writing them.",
}
