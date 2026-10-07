# luafmt

```sh
luafmt file.lua directory/
luafmt --check directory/
luafmt -eq --stdin path/to/file.lua < file.lua
luafmt --dialect lua54 file.lua
luafmt --completions zsh
dotfile dev check -p luafmt
```

| Behavior | Value |
| --- | --- |
| CLI | Shared formatter flags, theme, reports and completions; `--help` lists flags |
| Directory inputs | `.lua` and `.luau`, case insensitive; shared walker skips build/vendor trees and symlinks |
| Explicit files | Any extension; symlinks retain their target relationship |
| Config priority | Nearest ancestor `luafmt.dotfile`, then `$XDG_CONFIG_HOME/luafmt/luafmt.dotfile` (default `~/.config`), then `~/luafmt.dotfile`, then built-in defaults |
| Config inheritance | Nearest file replaces parent settings; unspecified settings use built-in defaults |
| Filters | `whitelist` and `blacklist` blocks use gitignore patterns; blacklist wins; paths relative to config directory, or working directory for global config |
| Writes | Atomic replacement, permissions retained, unchanged files untouched |
| Engine | Embedded [StyLua](https://github.com/JohnnyMorganz/StyLua); no subprocess |
| Performance | Parallel files, shared parsed configs and compiled globs, one resolution per directory; `RAYON_NUM_THREADS` limits workers |
| Ignore comments | `-- stylua: ignore`, `-- stylua: ignore start`, `-- stylua: ignore end` |
| Encoding | UTF-8; an existing UTF-8 BOM is preserved |
| LuaJIT parser limitations | Valid hexadecimal floats such as `0x1p-1026` and identifiers named `goto` currently produce parse errors; affected files remain unchanged |

Settings live in a `luafmt { ... }` block. See [shipped defaults](../../../../../shared/tools/luafmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `dialect` | `auto` | `auto`, `all`, `lua51`, `lua52`, `lua53`, `lua54`, `luajit`, `luau`; auto selects Luau for `.luau`, Lua 5.4 otherwise; CLI overrides config |
| `width` | `120` | 1–10000; preferred width, not a hard limit |
| `indent` | `2` | 1–16; spaces per level or visual tab width |
| `indent_type` | `spaces` | `spaces`, `tabs` |
| `line_endings` | `unix` | `unix`, `windows` |
| `quote_style` | `auto-prefer-double` | `auto-prefer-double`, `auto-prefer-single`, `force-double`, `force-single` |
| `call_parentheses` | `none` | `always`, `no-single-string`, `no-single-table`, `none`, `input` |
| `collapse_simple_statement` | `never` | `never`, `function-only`, `conditional-only`, `always` |
| `space_after_function_names` | `never` | `never`, `definitions`, `calls`, `always` |
| `block_newline_gaps` | `never` | `never`, `preserve` |
| `sort_requires` | `false` | Boolean; enable only when reordering module loads is safe |
| `final_newline` | `false` | Boolean |
| `verify` | `false` | Boolean; reparse emitted output to validate syntax; adds processing cost; does not prove semantic equivalence |
