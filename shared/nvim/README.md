# Neovim configuration

| Location | Owns |
| --- | --- |
| `init.lua` | Loads root configuration once; connects settings to runtime modules |
| `editor.lua` | Behavior, timings, completion preferences, commands, autocmds |
| `keymap.lua` | Global, buffer, picker and plugin key assignments |
| `plugins.lua` | Lazy specs, dependencies, load triggers, native option assembly |
| `lua/ui/init.lua` | Icons, diagnostics display, borders, dimensions, picker appearance |
| `lua/ui/theme.lua` | Generated palette; source: `theme/profiles/`, `theme/maps/nvim.toml` |
| `lua/languages/init.lua` | Language catalog; derives tool lists and filetype mappings |
| `lua/languages/{lsp,format,lint,syntax}.lua` | Tool settings and lifecycle |
| `lua/utils/` | Editor integration, named actions, sessions, clipboard, search |
| `lua/lib/` | Pure Lua abstractions; no Neovim, plugins, configuration or I/O |

| Change | Source |
| --- | --- |
| Format timeout | `editor.lua` → `formatting` |
| Completion documentation | `editor.lua` → `completion.documentation` |
| Completion keys | `keymap.lua` → `completion` |
| Terminal dimensions | `lua/ui/init.lua` → `terminal` |
| Language tools | `lua/languages/init.lua` → language entry |
| Server-specific options | `lua/languages/lsp.lua` → `configs` |
| Add/remove a plugin | `plugins.lua`; remove its mappings from `keymap.lua` |

| Boundary | Rule |
| --- | --- |
| Configuration | Declare preferences once; pass them into runtime setup |
| Runtime modules | Never load root configuration or UI settings back through `require`/`dofile` |
| Plugin APIs | Load inside setup or action callbacks, after Lazy bootstrap |
| Language catalog | Uses native tool names; LSP defaults retain their supported filetypes |
| Modules | Split substantial responsibilities; keep related callbacks together |
