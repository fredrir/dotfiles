from tools.theme.model import load_map, path

EZA_KINDS = ("fi", "di", "ex", "ln", "pi", "so", "bd", "cd")
OUTPUT = "shared/zsh/conf.d/03-theme.zsh"


def emit(theme, out):
    def escape(role):
        return f"$'\\e[{theme.ansi(theme.role(role))}m'"

    lines = [
        f"# {theme.header}",
        "export THEME_RESET=$'\\e[0m'",
        f"export THEME_SUDO={escape('sudo')}",
        f"export THEME_GIT={escape('prompt_git')}",
        f"export THEME_DIR={escape('prompt_dir')}",
        f"export THEME_CHAR={escape('prompt_char')}",
    ]
    eza = dict(theme.data.get("eza", {}))
    if eza:
        categories = eza.pop("categories", {})
        extensions = load_map("eza")["categories"]
        parts = ["reset"]
        for kind in EZA_KINDS:
            if kind in eza:
                parts.append(f"{kind}={theme.ansi(theme.hex(eza[kind]))}")
        for category, color in categories.items():
            for extension in extensions[category].split():
                parts.append(f"*.{extension}={theme.ansi(theme.hex(color))}")
        for key, color in eza.items():
            if key.startswith("*"):
                parts.append(f"{key}={theme.ansi(theme.hex(color))}")
        lines.append("unset LS_COLORS")
        lines.append(f'export EZA_COLORS="{":".join(parts)}"')
    out.write(path(OUTPUT), "\n".join(lines) + "\n")
