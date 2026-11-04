# dotfmt for VS Code

Formats with the bundled [dotfmt](../../../../docs/cli/dotfmt.md) and stays quiet unless dotfmt itself fails.

## Behaviour

- **Formatting:** Format Document, Format Selection, format on save and `editor.formatOnSaveMode` `modifications` all run `dotfmt --editor <file>` on the buffer. Selections apply only the changed lines they touch.
- **Selection:** the filename and `dotfmt.dotfile` decide the language and dialect, as in Neovim. Untitled buffers use their VS Code language through `--lang`.
- **Settings:** `dotfmt.dotfile` is the only source; VS Code's tab size and indentation settings are ignored. A `line_endings` change also switches the document's line endings.
- **Quiet:** a buffer that does not parse is left untouched without a message.
- **Errors:** a broken `dotfmt.dotfile`, I/O failures, internal guards and a missing or crashing binary show one notification per distinct error. Config errors also appear in Problems at their line, and every failure is logged to the dotfmt output.
- **Status:** the `{}` language status shows the resolved language and dialect, or why a file is skipped (`unmapped`, `excluded`, `disabled`), with a link to the nearest `dotfmt.dotfile`. It turns red on errors.
- **Dotfile language:** `*.dotfile` gets highlighting, `#` comments, bracket matching and indentation.
- **Coverage:** dotfmt's built-in filenames plus its languages' VS Code language ids. Files mapped only through an `include` rule need a matching `files.associations` entry.
- **Binary:** `dotfmt.path` replaces the bundled binary and is ignored in untrusted workspaces.

## Build and install

```sh
bun run deploy                       # builds darwin-arm64 and linux-x64 dotfmt, installs the vsix
bun run deploy -- --remote archie    # also installs it on a Remote SSH host
bun run build -- --host              # builds only for the current machine
```
