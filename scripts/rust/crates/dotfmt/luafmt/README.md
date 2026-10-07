# Lua engine

Internal library used by `dotfmt -l lua`. See [dotfmt](../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l lua file.lua directory/
dotfmt -l lua --check directory/
dotfmt -l lua -eq --stdin path/to/file.lua < file.lua
dotfmt -l lua --dialect lua54 file.lua
dotfile dev check -p luafmt
```

| Behavior | Value |
| --- | --- |
| Engine | Embedded [StyLua](https://github.com/JohnnyMorganz/StyLua); no subprocess |
| Ignore comments | `-- stylua: ignore`, `-- stylua: ignore start`, `-- stylua: ignore end` |
| Encoding | UTF-8; an existing UTF-8 BOM is preserved |
| LuaJIT parser limitations | Hexadecimal floats such as `0x1p-1026` and identifiers named `goto` produce parse errors; affected files remain unchanged |
| Configuration | `lua { ... }` in `dotfmt.dotfile`; global values and local overrides inherit |

Engine defaults below apply before configuration; see [shipped configuration](../../../../../shared/tools/dotfmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `dialect` | `auto` | `auto`, `all`, `lua51`, `lua52`, `lua53`, `lua54`, `luajit`, `luau`; auto selects Luau for `.luau`, Lua 5.4 otherwise; CLI overrides config |
| `width` | `120` | 1–10000; preferred width, not a hard limit |
| `indent` | `2` | 1–16; spaces per level or visual tab width |
| `indent_type` | `spaces` | `spaces`, `tabs` |
| `line_endings` | `unix` | `unix`, `windows` |
| `quote_style` | `auto-prefer-double` | `auto`, `auto-prefer-double`, `auto-prefer-single`, `double` (`force-double`), `single` (`force-single`) |
| `call_parentheses` | `none` | `always`, `no-single-string`, `no-single-table`, `none`, `input` |
| `collapse_simple_statement` | `never` | `never`, `function-only`, `conditional-only`, `always` |
| `space_after_function_names` | `never` | `never`, `definitions`, `calls`, `always` |
| `block_newline_gaps` | `never` | `never`, `preserve` |
| `sort_requires` | `false` | Boolean; enable only when reordering module loads is safe |
| `final_newline` | `false` | Boolean |
| `verify` | `false` | Boolean; reparse emitted output to validate syntax; adds processing cost; does not prove semantic equivalence |
