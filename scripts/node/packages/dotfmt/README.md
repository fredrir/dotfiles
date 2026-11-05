# dotfmt for VS Code

dotfmt is the one formatter for config, data, markup, TypeScript/TSX, JavaScript/JSX, CSS and Tailwind. It is **not a linter**: the extension publishes only errors, never lint findings, hints, information or warnings.

Formatting, range formatting, on-type formatting, errors, code actions and commands all run in the bundled [dotfmt](../../../../docs/cli/dotfmt.md) language server over stdio. The extension activates when a supported language or a dotfmt command is used; it does not start at editor launch.

## Setup

- **Default formatter:** run **dotfmt: Set as Default Formatter**, or set it per language in `settings.json`. With a workspace open it writes workspace settings; without one it writes user settings.
- **Format on save:** enable `editor.formatOnSave`. `editor.formatOnSaveMode: "modifications"` and Format Selection work through the server's range requests.
- **Format on type:** `;` and `}` trigger formatting of the surrounding block.
- **Format on paste:** pasted text is formatted through the language server and offered as a paste edit; a snippet that does not parse keeps its original text. Paste formatting is format-only and never applies configured fixes.
- **Binary:** `dotfmt.server.path` replaces the bundled language server. `dotfmt.path` names the CLI used by status and doctor. Workspace values are ignored in untrusted workspaces.
- **Configuration:** the filename and `dotfmt.dotfile` decide the language and dialect, as in Neovim. Untitled buffers use their VS Code language and the engine's default dialect; JSONC and JSONL need a filename extension because an untitled JSON buffer formats as plain JSON. The nearest `dotfmt.dotfile` applies, with the user config directory as the final fallback. With no language block anywhere, the shipped default set is used (everything except JavaScript).
- **Settings:** `dotfmt.dotfile` is the base formatting configuration and every option also has a native setting under `dotfmt.format.*`; VS Code's tab size and indentation settings are ignored. A native value overrides `dotfmt.dotfile`, so the editor can change width or quote style without editing a dotfile.
- **Scope:** the dotfmt settings below are resource-scoped. In a multi-root workspace each folder resolves its own values from the nearest workspace folder, including its own type-aware scope and TypeScript SDK.
- **Status:** the dotfmt language status item shows the resolved language, dialect and config, or why a file is skipped (`unmapped`, `excluded`, `disabled`). Its tooltip shows the extension version, server name and version, CLI path, tsc state, the resolved compiler when the server reports it, and the configuration chain. It turns red on errors.
- **Dotfile language:** `*.dotfile` gets highlighting, `#` comments, bracket matching and indentation.

## Native VS Code settings

Every dotfmt option is available in Settings. Run **dotfmt: Configure** (or search `@ext:fredrir.dotfmt dotfmt.format`) to see the generated flat keys:

- `dotfmt.format.global.<option>`: global options such as `width`, `indent`, `final_newline`, `repair` and `quote_style`.
- `dotfmt.format.<language>.<option>`: language options such as `dotfmt.format.typescript.quote_style` and `dotfmt.format.json.indent`.
- `dotfmt.format.<language>.enabled`, `.include` and `.exclude`.
- `dotfmt.format.included_files` and `dotfmt.format.excluded_files`.
- `dotfmt.formatExtra`: a free-form object using the `dotfmt.dotfile` shape for anything not listed above.

Precedence, lowest to highest: engine defaults → global `dotfmt.dotfile` → ancestor/nearest `dotfmt.dotfile` → `dotfmt.formatExtra` → `dotfmt.format.*`. Inside the native layers `dotfmt.format.*` wins over `dotfmt.formatExtra` for the same key. Generated settings have a `null` VS Code default, so leaving a key unset inherits from `dotfmt.dotfile`; `null` inherits too.

```json
{
  "dotfmt.format.global.width": 100,
  "dotfmt.format.typescript.quote_style": "single",
  "dotfmt.format.json.indent": 2,
  "dotfmt.format.included_files": ["src/**"],
  "dotfmt.formatExtra": {
    "typescript": { "remove_unused_exports": true }
  }
}
```

Generated settings are constrained by the editor schema. `dotfmt.formatExtra` is an escape hatch: an invalid value is logged and reported as a configuration diagnostic for that resource until it is corrected.

## Diagnostics: errors only

The editor receives exactly two classes of diagnostics:

1. Fatal syntax and configuration errors: the formatter cannot parse the buffer or resolve its configuration. Disable with `dotfmt.diagnostics.syntax = false`; this stops syntax and configuration diagnostics only.
2. TypeScript compiler errors, when `dotfmt.typeAware` is on (the default). Disable per resource with `dotfmt.typeAware = false`.

There are **no lint diagnostics**. dotfmt never publishes `unused-import`, `type-only-import`, `unused-export`, `unused-variable`, `unused-parameter`, `unused-type-parameter` or `unused-class-member` findings, and it never uses `DiagnosticSeverity.Hint`, Information or Warning. Unused-code removal is a formatting/fix action that you opt into, not an editor warning.

The CLI is unchanged: `dotfmt --diagnose` still reports findings, `--fix` applies safe fixes, and `--fix-unsafe` applies all fixes.

## Opt-in fixes in `dotfmt.dotfile`

Formatting never deletes code by default. Enable individual fix categories, or choose a fix mode:

```toml
fixes = "off"                   # off | safe | unsafe
remove_unused_imports = false
remove_unused_exports = false
remove_unused_locals = false
remove_unused_parameters = false
convert_type_only_imports = false
organize_imports = false
```

- `fixes = "off"` applies no format-time fixes unless an individual boolean is true.
- `fixes = "safe"` implies `remove_unused_imports`, `convert_type_only_imports` and `organize_imports`.
- `fixes = "unsafe"` implies the safe fixes plus `remove_unused_locals`. It never implies `remove_unused_exports`; removing exports changes the public API and must be requested explicitly.
- `remove_unused_exports = true` removes project-wide unused exports during full-document formatting. It is never implied by `fixes` and requires a trusted workspace.
- `remove_unused_parameters` is diagnose-only: `dotfmt --diagnose` reports unused parameters and formatting leaves them alone because there is no parameter-removal fix yet.
- An individual boolean forces that category on regardless of `fixes`, except for diagnose-only `remove_unused_parameters`.
- Selected fixes run before the final format, for at most three idempotent passes. Defaults delete nothing.

Configured fixes apply only to full-document formatting and `textDocument/willSaveWaitUntil`. Range formatting, on-type formatting, paste formatting and formatting requested through a code action never apply them; explicit fix actions (`source.fixAll.dotfmt`, **dotfmt: Fix All**) apply fixes directly.

The implemented import and local-variable categories are available interactively through code actions and **dotfmt: Fix All**. Project-wide unused exports are offered as the unsafe quick fix **Remove unused export (unsafe)** when `dotfmt.codeActions.unusedExports` is on (the default) and the workspace is trusted; export removal is also available at format time when `remove_unused_exports = true`, including through `dotfmt --fix-unsafe` with that setting enabled. Parameter removal is diagnose-only. Unsafe fixes are hidden in untrusted workspaces.

## Type-aware diagnostics

`dotfmt.typeAware` (default `true`) runs the project's TypeScript compiler once per project and caches the result. It is the compatibility switch for tsc; there is no duplicate setting.

- `dotfmt.diagnostics.tscScope` (default `open`) publishes compiler errors only for open documents. Set it to `project` to publish errors for every file in the project.
- `dotfmt.diagnostics.debounceMs` (default `250`) coalesces checks after edits.
- `dotfmt.typescript.tsdk` accepts a TypeScript package directory (`node_modules/typescript`), its `lib` directory (`node_modules/typescript/lib`) or `tsc.js` itself. Empty auto-discovers `DOTFMT_TSC`, the project's `node_modules/typescript/lib/tsc.js`, `node_modules/.bin/tsc`, `@typescript/native-preview`/`tsgo`, then `tsc` on `PATH`.
- Compiler discovery and version are logged once per project. TypeScript's unused-code and lint codes (6133, 6138, 6192, 6196, 6198, 6199, 6205, 7027, 7028) are filtered out, so tsc errors never become unused-code warnings or hints.
- If the compiler fails or times out three times in a row, dotfmt backs off until the configuration or compiler changes and shows that state in status and doctor.
- Project checks, project JavaScript execution and unsafe fixes, including `remove_unused_exports`, require an explicitly trusted workspace. In an untrusted workspace they are disabled; deterministic formatting still works.

## Settings

| Setting                         | Type                                  | Default  | Scope               | Purpose                                                                                |
| ------------------------------- | ------------------------------------- | -------- | ------------------- | -------------------------------------------------------------------------------------- |
| `dotfmt.typeAware`              | boolean                               | `true`   | resource            | Run tsc for errors only; never lint findings                                           |
| `dotfmt.diagnostics.syntax`     | boolean                               | `true`   | resource            | Publish fatal syntax and configuration errors; `false` stops syntax/config diagnostics only (tsc errors remain) |
| `dotfmt.diagnostics.tscScope`   | `"open"` \| `"project"`              | `"open"` | resource            | Publish tsc errors for open documents or the whole project                             |
| `dotfmt.diagnostics.debounceMs` | number, 50..2000                      | `250`    | resource            | Delay before tsc checks rerun after an edit                                            |
| `dotfmt.typescript.tsdk`        | string                                | `""`     | resource            | TypeScript package dir, `lib` dir or `tsc.js`; empty auto-discovers                 |
| `dotfmt.fixes.onSave`           | `"off"` \| `"safe"` \| `"unsafe"`     | `"off"`  | resource            | Apply configured fixes through `willSaveWaitUntil` and full-document formatting        |
| `dotfmt.codeActions.unusedExports` | boolean                            | `true`   | resource            | Offer **Remove unused export (unsafe)** quick fixes from project-wide analysis; trusted workspaces only |
| `dotfmt.format.*`               | generated                             | unset    | resource            | Native overlay for every formatting option; unset inherits `dotfmt.dotfile`             |
| `dotfmt.formatExtra`            | object                                | `{}`     | resource            | Free-form overlay for advanced options; lower precedence than `dotfmt.format.*`         |
| `dotfmt.path`                   | string                                | `""`     | machine-overridable | CLI binary used by status and doctor; empty uses the bundled one                       |
| `dotfmt.server.path`            | string                                | `""`     | machine-overridable | Language server binary; empty uses the bundled one                                     |
| `dotfmt.trace.server`           | `"off"` \| `"messages"` \| `"verbose"` | `"off"`  | window              | Route LSP tracing to the **dotfmt trace** output channel; set that channel's log level to Trace |

```json
{
  "dotfmt.typeAware": true,
  "dotfmt.diagnostics.tscScope": "open",
  "dotfmt.fixes.onSave": "off",
  "[typescript]": {
    "editor.defaultFormatter": "fredrir.dotfmt",
    "editor.formatOnSave": true
  }
}
```

## Commands

| Command                        | Purpose                                                                                |
| ------------------------------ | -------------------------------------------------------------------------------------- |
| `dotfmt.fixAll`                | Apply safe fixes and format the active file                                            |
| `dotfmt.organizeImports`       | Organize imports and format                                                            |
| `dotfmt.sortTailwind`          | Format, including Tailwind class order                                                 |
| `dotfmt.restartServer`         | Restart the language server                                                            |
| `dotfmt.showOutput`            | Show the dotfmt output channel                                                         |
| `dotfmt.showStatus`            | Show the active file's resolved language, configuration, tsc state and server version   |
| `dotfmt.setAsDefaultFormatter` | Set dotfmt as the default formatter for its languages                                  |
| `dotfmt.configure`             | Open Settings filtered to every native `dotfmt.format.*` option                         |
| `dotfmt.openConfig`            | Open or create the nearest `dotfmt.dotfile`; without a local one, open an existing global file before prompting |
| `dotfmt.doctor`                | Print the server report, paths, versions, trust, settings, tsc state and config chain; copy summary |

**dotfmt: Doctor** calls the server's `dotfmt/doctor` request and reports the server name and version, trust, type-aware and syntax settings, tsc scope, the default and per-folder resolved settings, workspace roots, and every resolved project's `tsconfig`, compiler, version and state. The extension adds its own version, server and CLI paths, workspace folders, settings and the configuration chain from `dotfmt/explain`. If an older server does not implement the request, doctor falls back to the client-known summary.

## Code actions

The server offers quick fixes for dotfmt findings. Unsafe fixes are only listed explicitly and carry `(unsafe)` in the title. Full-file actions use versioned workspace edits and change annotations:

- `source.fixAll.dotfmt`: all safe fixes, then formatting.
- `source.organizeImports.dotfmt`: import organization and formatting.
- `source.format.dotfmt`: formatting only; it never applies fixes.

A quick-fix request for a saved TypeScript/JavaScript file can also offer **Remove unused export (unsafe)** from project-wide analysis. That analysis runs only for code-action requests, only when `dotfmt.codeActions.unusedExports` is on and the workspace is trusted, and it never publishes diagnostics.

Run them from the lightbulb menu, **dotfmt: Fix All**, **dotfmt: Organize Imports** and **dotfmt: Sort Tailwind Classes**, or on save:

```json
{
  "editor.codeActionsOnSave": {
    "source.fixAll.dotfmt": "explicit",
    "source.organizeImports.dotfmt": "explicit"
  }
}
```

The unsuffixed `source.fixAll` and `source.organizeImports` kinds are deliberately not advertised, so other formatters and linters keep their own save actions. `dotfmt.fixes.onSave` is an alternative that runs fixes through `textDocument/willSaveWaitUntil` before the file is written (`off`, `safe` or `unsafe`; default `off`).

## Tailwind

Tailwind class sorting is part of formatting when the applicable `dotfmt.dotfile` enables it; **dotfmt: Sort Tailwind Classes** runs the same formatter path. Tailwind order can come from the project stylesheet or the built-in order, as configured in `dotfmt.dotfile`.

In untrusted workspaces the server disables project JavaScript execution for Tailwind ordering, unsafe fixes and `tsc`; deterministic Rust Tailwind ordering still works.

## Coverage

dotfmt's built-in filenames plus languages for TypeScript, TSX, JavaScript, JSX, CSS, SCSS, Sass, Less, HTML, Vue, Svelte, Astro and the existing config/data languages. Files mapped only through an `include` rule need a matching `files.associations` entry.

## Build and install

```sh
bun run deploy                          # build linux-x64 + darwin-arm64, package the universal vsix, install locally
bun run deploy -- --host                # build only the current platform, package it with vsce --target
bun run deploy -- --skip-build          # package/install existing bin/ and dist/ artifacts
bun run deploy -- --target linux-x64    # build and package one target with vsce --target
bun run deploy -- --remote archie       # also install on a Remote SSH host with a matching platform
bun run build -- --host                 # build only the current machine
bun run build -- --target linux-x64 --target darwin-arm64
```

`build.ts` builds only `linux-x64` and `darwin-arm64` by default. `win32-x64` remains mapped for an explicit `--target` or `--host` on Windows; it is not part of the default build. `deploy.ts` reads the **remote** host's `~/.vscode-server/cli/servers/lru.json` and prefers the directory whose commit matches `ssh <host> code --version`, so a remote install never uses the local desktop commit. Before uploading, it compares `ssh <host> uname -s -m` with the platforms in the packaged vsix and refuses when no matching server binary ships; use `--target` for one platform or the default universal package, which contains `linux-x64` and `darwin-arm64`. `--skip-build` leaves the existing `bin/` and `dist/` untouched.

Run the client typecheck from `scripts/node`:

```sh
bun x tsc -p packages/dotfmt/tsconfig.json --noEmit
```
