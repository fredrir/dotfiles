# dotfmt for VS Code

Formats with the bundled [dotfmt](../../../../docs/cli/dotfmt.md) language server and stays quiet unless dotfmt itself fails.

## Setup

The extension starts `bin/<platform>/dotfmt-lsp` and speaks LSP over stdio. Formatting, range formatting, on-type formatting, diagnostics, fixes and commands all run in that one process; the VS Code side only wires the client, status item and commands.

- **Default formatter:** run **dotfmt: Set as Default Formatter**, or set it per language in `settings.json`.
- **Format on save:** enable `editor.formatOnSave`. `editor.formatOnSaveMode: "modifications"` and Format Selection work through the server's range requests.
- **Format on type:** `;` and `}` trigger formatting of the surrounding block.
- **Format on paste:** pasted text is formatted through the language server and offered as a paste edit; a broken snippet keeps its original text.
- **Binary:** `dotfmt.server.path` replaces the bundled language server. `dotfmt.path` names the CLI shown in the output log for manual checks. Workspace values are ignored in untrusted workspaces.
- **Configuration:** the filename and `dotfmt.dotfile` decide the language and dialect, as in Neovim. Untitled buffers use their VS Code language.
- **Settings:** `dotfmt.dotfile` is the only formatting configuration; VS Code's tab size and indentation settings are ignored. A `line_endings` change also switches the document's line endings.
- **Quiet:** a buffer that does not parse is left untouched and only gets a diagnostic. Formatting never shows a popup for syntax errors; failures are logged to the dotfmt output channel.
- **Status:** the `{}` language status shows the resolved language and dialect, or why a file is skipped (`unmapped`, `excluded`, `disabled`), with a link to the nearest `dotfmt.dotfile`. It turns red on errors.
- **Dotfile language:** `*.dotfile` gets highlighting, `#` comments, bracket matching and indentation.

## Code actions

The server offers quick fixes for dotfmt findings. Unsafe fixes are only listed explicitly and carry `(unsafe)` in the title. Full-file actions use versioned workspaces edits and change annotations:

- `source.fixAll.dotfmt`: all safe fixes, then formatting.
- `source.organizeImports.dotfmt`: import organization and formatting.
- `source.format.dotfmt`: formatting only.

Run them from the lightbulb menu, **dotfmt: Fix All**, **dotfmt: Organize Imports** and **dotfmt: Sort Tailwind Classes**, or on save:

```json
{
  "editor.codeActionsOnSave": {
    "source.fixAll.dotfmt": "explicit",
    "source.organizeImports.dotfmt": "explicit"
  }
}
```

The unsuffixed `source.fixAll` and `source.organizeImports` kinds are deliberately not advertised, so other formatters and linters keep their own save actions. `dotfmt.fixes.onSave` is an alternative that runs `dotfmt.fixAll` through `textDocument/willSaveWaitUntil`, before the file is written (`off`, `safe` or `unsafe`; default `off`). Unsafe fixes are disabled in untrusted workspaces.

## Tailwind and type-aware checks

Tailwind class sorting is part of formatting when the applicable `dotfmt.dotfile` enables it; **dotfmt: Sort Tailwind Classes** runs the same formatter path. Tailwind order can come from the project stylesheet or the built-in order, as configured in `dotfmt.dotfile`.

In untrusted workspaces the server sets `DOTFMT_TRUSTED=0`, which disables project JavaScript execution for Tailwind ordering, unsafe fixes and `tsc`; deterministic Rust Tailwind ordering still works.

`dotfmt.typeAware` (default on) runs `tsc --noEmit --pretty false --project <nearest tsconfig>` in a worker thread when a TypeScript or JavaScript file is opened or saved. Project diagnostics appear with source `dotfmt(tsc)` and never block formatting. Results are cached by project and file mtimes with a timeout; the check is skipped in untrusted workspaces.

## Commands and settings

| Command                        | Purpose                                                 |
| ------------------------------ | ------------------------------------------------------- |
| `dotfmt.fixAll`                | Apply safe fixes and format the active file             |
| `dotfmt.organizeImports`       | Organize imports and format                             |
| `dotfmt.sortTailwind`          | Format, including Tailwind class order                  |
| `dotfmt.restartServer`         | Restart the language server                             |
| `dotfmt.showOutput`            | Show the dotfmt output channel                          |
| `dotfmt.setAsDefaultFormatter` | Set dotfmt as the workspace formatter for its languages |
| `dotfmt.openConfig`            | Open or create the nearest `dotfmt.dotfile`             |

`dotfmt.trace.server` (`off`, `messages`, `verbose`) traces LSP traffic.

## Coverage

dotfmt's built-in filenames plus languages for TypeScript, TSX, JavaScript, JSX, CSS, SCSS, Sass, Less, HTML, Vue, Svelte, Astro and the existing config/data languages. Files mapped only through an `include` rule need a matching `files.associations` entry.

## Build and install

```sh
bun run deploy                       # builds dotfmt and dotfmt-lsp for darwin-arm64 and linux-x64, installs the vsix
bun run deploy -- --remote archie    # also installs it on a Remote SSH host
bun run build -- --host              # builds only for the current machine
```

`build.ts` also maps `win32-x64`; install the Rust target and run `bun build.ts --host` from Windows to build it.
