# dotfmt-lsp

A stdio language server for the dotfmt VS Code extension and any LSP client.

- `textDocument/formatting`, `rangeFormatting`, LSP 3.18 `rangesFormatting`,
  `onTypeFormatting`, push diagnostics, code actions, execute commands and
  `willSaveWaitUntil`.
- Quiet by design: syntax errors and `tsc` failures are diagnostics, not
  popups; document versions are checked before edits are returned.
- Code action kinds: `quickfix`, `source.fixAll.dotfmt`,
  `source.organizeImports.dotfmt`, `source.format.dotfmt`. The unsuffixed
  `source.*` kinds are deliberately not claimed so VS Code's TypeScript
  extension keeps its own actions.
- Project-wide type diagnostics run `tsc --noEmit --pretty false --project`
  on a worker with a timeout and a mtime cache. `dotfmt.typeAware` disables
  them; untrusted workspaces skip them.
- `dotfmt.fixes.onSave` is applied through `willSaveWaitUntil`, before the file
  is written.
- Commands: `dotfmt.fixAll`, `dotfmt.organizeImports`, `dotfmt.sortTailwind`,
  `dotfmt.explain`, `dotfmt/reloadConfig`; custom requests
  `dotfmt/explain` and `dotfmt/formatText` (paste).

```sh
dotfmt-lsp            # stdio; VS Code extension starts it for you
cargo test -p dotfmt-lsp
```
