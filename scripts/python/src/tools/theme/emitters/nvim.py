from tools.theme.model import load_map, path

OUTPUT = "shared/nvim/lua/plugins/catppuccin.lua"


def emit(theme, out):
    spec = load_map("nvim")
    flavour = spec["flavour"]["dark" if theme.dark else "light"]
    colors = {name: theme.hex(value) for name, value in spec["colors"].items()}

    def transform(text):
        unit = "\t" if "\n\t" in text else "  "
        body = [f'flavour = "{flavour}",', "color_overrides = {", f"{unit}all = {{"]
        body += [f'{unit * 2}{name} = "{value}",' for name, value in colors.items()]
        body += [f"{unit}}},", "},"]
        lines = text.split("\n")
        first = next(
            (index for index, line in enumerate(lines) if line.lstrip().startswith("flavour =")),
            None,
        )
        if first is None:
            raise SystemExit(f"dotfile theme: {OUTPUT}: 'flavour' setting not found")
        table = first + 1
        while table < len(lines) and not lines[table].strip():
            table += 1
        if table == len(lines) or not lines[table].lstrip().startswith("color_overrides = {"):
            raise SystemExit(f"dotfile theme: {OUTPUT}: 'color_overrides' must follow 'flavour'")
        depth = 0
        last = None
        for index in range(table, len(lines)):
            depth += lines[index].count("{") - lines[index].count("}")
            if depth == 0:
                last = index
                break
        if last is None:
            raise SystemExit(f"dotfile theme: {OUTPUT}: 'color_overrides' table is not closed")
        indent = lines[first][: len(lines[first]) - len(lines[first].lstrip())]
        rendered = [indent + line if line else line for line in body]
        return "\n".join(lines[:first] + rendered + lines[last + 1 :])

    out.edit(path(OUTPUT), transform)
