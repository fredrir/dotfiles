import os
import pwd
import re
import shutil
import tomllib

from tools.core import blocks
from tools.core.console import colors_enabled
from tools.core.process import capture
from tools.dotfile import adoption
from tools.dotfile import link as link_command
from tools.dotfile import merge as merge_state
from tools.dotfile.profiles import detect_platform
from tools.dotfile.report import (
    BOLD,
    DIM,
    INDENT,
    emit,
    paint,
    row,
)
from tools.dotfile.state import (
    collect_groups,
    die,
    load_overrides,
    log,
    manifest_groups,
    pending_overrides,
    require_manifest,
    resolve_profile,
    shorten,
    trim,
)
from tools.dotfile.targets import load_targets

KINDS = ("command", "font", "file")

STALE_BENCHMARK_DAYS = 120

FONT_DIRS = (
    "~/Library/Fonts",
    "/Library/Fonts",
    "/System/Library/Fonts",
    "~/.local/share/fonts",
    "~/.fonts",
    "/usr/share/fonts",
    "/usr/local/share/fonts",
)
FONT_SUFFIXES = (".ttf", ".otf", ".ttc", ".dfont", ".pfb")
FONT_WEIGHTS = {
    "",
    "regular",
    "normal",
    "book",
    "text",
    "thin",
    "extralight",
    "ultralight",
    "light",
    "medium",
    "retina",
    "semibold",
    "demibold",
    "bold",
    "extrabold",
    "ultrabold",
    "black",
    "heavy",
}

PLUGIN_PATTERNS = (
    re.compile(r"\$ZSH/custom/plugins/([A-Za-z0-9._-]+)"),
    re.compile(r"/([A-Za-z0-9._-]+)/\1(?:\.plugin)?\.zsh"),
)

PLUGIN_DIRS = ("/usr/share/zsh/plugins", "/usr/share", "/usr/local/share")

LINK_GLYPHS = {
    merge_state.ADD: "+",
    merge_state.MODIFY: "~",
    merge_state.DELETE: "-",
    merge_state.CONFLICT: "!",
}
LINK_STATE_WIDTH = 10


def read_requirement_entries(ctx):
    try:
        return blocks.read(ctx.requires_file)
    except blocks.BlockError as error:
        die(blocks.describe(error, "config/requirements.dotfile", "group"))
        return []


def load_requirements(ctx):
    groups = {}
    if not os.path.isfile(ctx.requires_file):
        return groups
    for entry in read_requirement_entries(ctx):
        if entry.opens:
            if not entry.block:
                die(f"config/requirements.dotfile:{entry.number}: empty group")
            if not os.path.isdir(os.path.join(ctx.root, entry.block)):
                die(f"config/requirements.dotfile:{entry.number}: unknown group: {entry.block}")
            groups.setdefault(entry.block, [])
            continue
        name, package = entry.split("=")
        optional = name.startswith("?")
        if optional:
            name = trim(name[1:])
        kind = "command"
        for word in KINDS[1:]:
            if name.startswith(word + " "):
                kind = word
                name = trim(name[len(word) :])
        if not name:
            die(f"config/requirements.dotfile:{entry.number}: empty entry")
        groups[entry.block].append((kind, name, package or name, optional))
    return groups


def read_pin_entries(ctx):
    try:
        return blocks.read(ctx.pins_file)
    except blocks.BlockError as error:
        die(blocks.describe(error, "config/pins.dotfile", "group"))
        return []


def load_pins(ctx):
    groups = {}
    if not os.path.isfile(ctx.pins_file):
        return groups
    for entry in read_pin_entries(ctx):
        if entry.opens:
            if not entry.block:
                die(f"config/pins.dotfile:{entry.number}: empty group")
            if not os.path.isdir(os.path.join(ctx.root, entry.block)):
                die(f"config/pins.dotfile:{entry.number}: unknown group: {entry.block}")
            groups.setdefault(entry.block, [])
            continue
        name, want = entry.split("=")
        if not name or not want:
            die(f"config/pins.dotfile:{entry.number}: expected `command = build`")
        groups[entry.block].append((name, want))
    return groups


def pin_version(name):
    result = capture([name, "--version"])
    if result.returncode != 0 or not result.stdout:
        return ""
    return trim(result.stdout.splitlines()[0])


def pin_rows(ctx, groups, show_all):
    pins = load_pins(ctx)
    entries = [pin for group in groups for pin in pins.get(group, [])]
    if not entries:
        return [], 0
    wrong = []
    for name, want in entries:
        if shutil.which(name) is None:
            wrong.append((name, f"not installed, want {want}"))
            continue
        have = pin_version(name)
        if want not in have:
            wrong.append((name, f"{have or 'no version output'}, want {want}"))
    if wrong:
        return [row("bad", "pins", f"{len(wrong)} mismatched", wrong, show_all)], len(wrong)
    return [row("ok", "pins", f"{len(entries)} pinned")], 0


def read_lines(path):
    with open(path, encoding="utf-8") as handle:
        return handle.read().splitlines()


def quoted(value):
    for quote in ('"', "'"):
        if quote in value:
            return value.partition(quote)[2].partition(quote)[0]
    return ""


def read_brewfile(path):
    names = []
    for raw in read_lines(path):
        line = trim(raw.split("#", 1)[0])
        if not line.startswith(("brew ", "cask ")):
            continue
        name = quoted(line)
        if name:
            names.append(name.rpartition("/")[2])
    return names


def read_pkglist(path):
    return [line for line in (trim(raw.split("#", 1)[0]) for raw in read_lines(path)) if line]


def brew_installed():
    names = set()
    for kind in ("--formula", "--cask"):
        result = capture(["brew", "list", kind, "-1"])
        if result.returncode == 0:
            names.update(result.stdout.split())
    return names


def pacman_installed():
    result = capture(["pacman", "-Qq"])
    if result.returncode != 0:
        return set()
    return set(result.stdout.split())


MANAGERS = {"brew": brew_installed, "pacman": pacman_installed}


def font_key(name):
    return "".join(character for character in name.lower() if character.isalnum())


def fonts_from_fc_list():
    result = capture(["fc-list", "--format", "%{family}\\n"])
    if result.returncode != 0:
        return set()
    return {font_key(family) for line in result.stdout.splitlines() for family in line.split(",")}


def fonts_from_directories():
    names = set()
    for directory in FONT_DIRS:
        path = os.path.expanduser(directory)
        if not os.path.isdir(path):
            continue
        for _parent, _dirnames, filenames in os.walk(path):
            for filename in filenames:
                if filename.lower().endswith(FONT_SUFFIXES):
                    names.add(font_key(os.path.splitext(filename)[0]))
    return names


def installed_fonts():
    if shutil.which("fc-list"):
        families = fonts_from_fc_list()
        if families:
            return families
    return fonts_from_directories()


def is_style(suffix):
    for slant in ("italic", "oblique"):
        suffix = suffix.removesuffix(slant)
    return suffix in FONT_WEIGHTS


def font_missing(family, installed):
    key = font_key(family)
    return not any(name.startswith(key) and is_style(name[len(key) :]) for name in installed)


def wrong_platform(ctx, profile, platform):
    if not platform or not os.path.isdir(os.path.join(ctx.environment_dir, platform)):
        return ""
    if profile == platform or profile.startswith(platform + "/"):
        return ""
    return f"not a {platform} profile"


def project_commands(ctx):
    project = os.path.join(ctx.root, "scripts", "python", "pyproject.toml")
    if not os.path.isfile(project):
        return []
    with open(project, "rb") as handle:
        data = tomllib.load(handle)
    return sorted(data.get("project", {}).get("scripts", {}))


def commands_problem(ctx):
    commands = project_commands(ctx)
    if not commands:
        return ""
    bindir = os.path.join(ctx.home, ".local", "bin")
    entries = [entry for entry in os.environ.get("PATH", "").split(os.pathsep) if entry]
    if os.path.realpath(bindir) not in [os.path.realpath(entry) for entry in entries]:
        return "~/.local/bin is not on PATH"
    missing = [name for name in commands if not os.path.isfile(os.path.join(bindir, name))]
    if missing:
        return "missing from ~/.local/bin: " + ", ".join(missing)
    data_home = os.environ.get("XDG_DATA_HOME") or os.path.join(ctx.home, ".local", "share")
    tool_dir = os.environ.get("UV_TOOL_DIR") or os.path.join(data_home, "uv", "tools")
    tool_root = os.path.realpath(tool_dir) + os.sep
    unmanaged = [
        name
        for name in commands
        if not os.path.realpath(os.path.join(bindir, name)).startswith(tool_root)
    ]
    if unmanaged:
        return "not installed by uv: " + ", ".join(unmanaged)
    shadowed = [
        name
        for name in commands
        if os.path.realpath(shutil.which(name) or "")
        != os.path.realpath(os.path.join(bindir, name))
    ]
    if shadowed:
        return "shadowed on PATH: " + ", ".join(shadowed)
    return ""


def login_shell():
    try:
        return pwd.getpwuid(os.getuid()).pw_shell
    except KeyError:
        return ""


def environment_rows(ctx, profile, manifest, platform):
    rows = []
    warning = wrong_platform(ctx, profile, platform)
    if warning:
        rows.append(row("warn", "profile", f"{warning} (host {platform})"))
    problem = commands_problem(ctx)
    if problem:
        rows.append(row("warn", "commands", problem))
    pending = pending_overrides(ctx, manifest)
    if pending:
        rows.append(row("warn", "overrides", "unselected for " + " ".join(pending)))
    shell = login_shell()
    if shell and os.path.basename(shell) != "zsh":
        rows.append(row("warn", "shell", f"login shell is {shell}, not zsh"))
    return rows, len(rows)


def profile_requirements(requirements, groups):
    entries = {}
    for group in groups:
        for kind, name, package, optional in requirements.get(group, []):
            previous = entries.get((kind, name))
            if previous is None or (previous[3] and not optional):
                entries[(kind, name)] = (kind, name, package, optional)
    return list(entries.values())


def requirement_rows(ctx, groups, show_all):
    entries = profile_requirements(load_requirements(ctx), groups)
    fonts = installed_fonts() if any(kind == "font" for kind, _n, _p, _o in entries) else set()

    missing = {kind: [] for kind in KINDS}
    counted = {kind: 0 for kind in KINDS}
    absent = []
    for kind, name, package, optional in entries:
        if kind == "command":
            gone = shutil.which(name) is None
        elif kind == "font":
            gone = font_missing(name, fonts)
        else:
            gone = not os.path.exists(os.path.expanduser(name))
        hint = package if package != name else ""
        if optional:
            if gone:
                absent.append((name, hint))
            continue
        counted[kind] += 1
        if gone:
            missing[kind].append((name, hint))

    rows = []
    for kind, label in (("command", "tools"), ("font", "fonts"), ("file", "files")):
        if missing[kind]:
            rows.append(row("bad", label, f"{len(missing[kind])} missing", missing[kind], show_all))
        elif counted[kind]:
            rows.append(row("ok", label, f"{counted[kind]} installed"))
    if absent:
        rows.append(row("note", "optional", f"{len(absent)} absent", absent, show_all))
    return rows, sum(len(missing[kind]) for kind in KINDS)


def declared_plugins(ctx, groups):
    names = set()
    for group in groups:
        directory = os.path.join(ctx.root, group, "zsh")
        for parent, _dirnames, filenames in os.walk(directory):
            for filename in filenames:
                if not filename.endswith((".zsh", "zshrc")):
                    continue
                for line in read_lines(os.path.join(parent, filename)):
                    for pattern in PLUGIN_PATTERNS:
                        names.update(pattern.findall(line))
    return sorted(names)


def omz_dir(ctx):
    return os.environ.get("ZSH") or os.path.join(ctx.home, ".oh-my-zsh")


def plugin_dirs(ctx, name):
    dirs = [os.path.join(omz_dir(ctx), "custom", "plugins", name)]
    dirs.extend(os.path.join(parent, name) for parent in PLUGIN_DIRS)
    brew = os.environ.get("HOMEBREW_PREFIX") or shutil.which("brew")
    if brew:
        prefix = brew if os.path.isdir(brew) else os.path.dirname(os.path.dirname(brew))
        dirs.append(os.path.join(prefix, "share", name))
    return dirs


def plugin_rows(ctx, groups, show_all):
    plugins = declared_plugins(ctx, groups)
    if not plugins:
        return [], 0
    if not os.path.isdir(omz_dir(ctx)):
        return [row("bad", "oh-my-zsh", f"not installed at {shorten(ctx, omz_dir(ctx))}")], 1
    missing = [
        (name, "")
        for name in plugins
        if not any(os.path.isdir(directory) for directory in plugin_dirs(ctx, name))
    ]
    if not missing:
        return [row("ok", "plugins", f"{len(plugins)} installed")], 0
    return [row("bad", "plugins", f"{len(missing)} missing", missing, show_all)], len(missing)


def package_sources(ctx, profile, groups):
    sources = []
    brewfile = os.path.join(ctx.root, "macos", "Brewfile")
    if "macos" in groups and os.path.isfile(brewfile):
        sources.append(("brewfile", "brew", read_brewfile(brewfile)))
    for name in ("pkglist.txt", "aurlist.txt"):
        path = os.path.join(ctx.environment_dir, profile, name)
        if os.path.isfile(path):
            sources.append((name.partition(".")[0], "pacman", read_pkglist(path)))
    return sources


def package_rows(ctx, profile, groups, show_all):
    rows = []
    cache = {}
    skipped = set()
    total = 0
    for label, manager, wanted in package_sources(ctx, profile, groups):
        if not shutil.which(manager):
            if manager not in skipped:
                skipped.add(manager)
                rows.append(row("note", manager, "not installed, package lists skipped"))
            continue
        if manager not in cache:
            cache[manager] = MANAGERS[manager]()
        missing = [(name, "") for name in wanted if name not in cache[manager]]
        if missing:
            rows.append(row("bad", label, f"{len(missing)} missing", missing, show_all))
        else:
            rows.append(row("ok", label, f"{len(wanted)} installed"))
        total += len(missing)
    return rows, total


def benchmark_rows(ctx):
    try:
        from tools.utils.sysinfo.bench import store
        from tools.utils.sysinfo.bench.health import age_in_days
        from tools.utils.sysinfo.hosts import resolve
    except ImportError:
        return [], 0
    try:
        host = resolve()
    except blocks.BlockError as error:
        return [row("bad", "benchmark", blocks.describe(error, "config/hosts.dotfile", "host"))], 1
    if not host:
        return [], 0
    runs = store.list_runs(host, grades=("clean",))
    if not runs:
        return [row("note", "benchmark", "no runs recorded for " + host)], 0
    age = age_in_days(runs[0])
    if age is None:
        return [], 0
    if age >= STALE_BENCHMARK_DAYS:
        return [row("warn", "benchmark", f"last clean run was {age} days ago")], 1
    noun = "run" if len(runs) == 1 else "runs"
    return [row("ok", "benchmark", f"{len(runs)} clean {noun}, newest {age} days old")], 0


def link_items(ctx, findings):
    width = max(shutil.get_terminal_size().columns - LINK_STATE_WIDTH - 12, 20)
    items = []
    for state, dst, detail, changes in findings:
        suffix = "  " + detail if detail and not changes else ""
        items.append((f"{state:<{LINK_STATE_WIDTH}} {shorten(ctx, dst)}{suffix}", ""))
        for change in changes[: merge_state.DETAIL_KEYS]:
            glyph = LINK_GLYPHS.get(change.kind, "|")
            rendered = adoption.render_change(change, width)
            key = change.key() or "(document)"
            suffix = "  " + rendered if rendered else ""
            items.append((f"{'':<{LINK_STATE_WIDTH}} {glyph} {key}{suffix}", ""))
        rest = len(changes) - merge_state.DETAIL_KEYS
        if rest > 0:
            items.append((f"{'':<{LINK_STATE_WIDTH}} … and {rest} more", ""))
    return items


def link_rows(ctx, show_all):
    load_targets(ctx)
    merge_state.load(ctx)

    linked = missing = differing = 0
    findings = []
    for state, dst, detail, changes in link_command.scan_links(ctx):
        if state in ("ok", merge_state.CURRENT):
            linked += 1
            continue
        findings.append((state, dst, detail, changes))
        if state == "missing":
            missing += 1
        elif state == merge_state.FORMATTING:
            linked += 1
        else:
            differing += 1

    summary = f"{linked} linked, {missing} missing, {differing} differing"
    kind = "bad" if missing + differing else "ok"
    return [row(kind, "links", summary, link_items(ctx, findings), show_all)], missing + differing


def cmd_doctor(ctx, profile, show_all):
    profile = resolve_profile(ctx, profile or "")
    manifest = require_manifest(ctx, profile)
    load_overrides(ctx)
    collect_groups(ctx, manifest, notes=False)
    groups = manifest_groups(manifest)

    rows = []
    problems = 0
    for section, count in (
        link_rows(ctx, show_all),
        environment_rows(ctx, profile, manifest, detect_platform()),
        requirement_rows(ctx, groups, show_all),
        pin_rows(ctx, groups, show_all),
        plugin_rows(ctx, groups, show_all),
        package_rows(ctx, profile, groups, show_all),
        benchmark_rows(ctx),
    ):
        rows.extend(section)
        problems += count

    color_on = colors_enabled()
    log("")
    log(INDENT + paint("doctor", DIM, color_on) + "  " + paint(profile, BOLD, color_on))
    log("")
    for entry in rows:
        emit(entry, color_on)
    log("")
    if problems:
        raise SystemExit(1)
