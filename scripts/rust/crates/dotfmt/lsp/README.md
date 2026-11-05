# dotfmt-lsp

A stdio language server for the dotfmt VS Code extension and any LSP client.

- `textDocument/formatting`, `rangeFormatting`, LSP 3.18 `rangesFormatting`,
  `onTypeFormatting`, push/pull diagnostics, code actions, execute commands and
  `willSaveWaitUntil`. Range edits use a stable original-to-formatted mapping;
  blank/EOF selections cannot expand to the whole file.
- Quiet by design: syntax errors and `tsc` failures are diagnostics, not
  popups; document versions are checked before edits are returned.
- Code action kinds: `quickfix`, `source.fixAll.dotfmt`,
  `source.organizeImports.dotfmt`, `source.format.dotfmt`. The unsuffixed
  `source.*` kinds are deliberately not claimed so VS Code's TypeScript
  extension keeps its own actions.
- Project-wide type diagnostics run `tsc --noEmit --pretty false --project`
  on a worker with a timeout and a dependency-aware fingerprint. References,
  path aliases and linked workspace packages are expanded. `dotfmt.typeAware`
  disables them; untrusted workspaces skip them.
- `dotfmt.fixes.onSave` is applied through `willSaveWaitUntil`, before the file
  is written. Safe fixes also run on Format Document untrusted; unsafe/export
  categories require trust.
- Commands: `dotfmt.fixAll`, `dotfmt.organizeImports`, `dotfmt.sortTailwind`,
  `dotfmt.explain`, `dotfmt/reloadConfig`; custom requests `dotfmt/explain`,
  `dotfmt/formatText` (paste, dialect-aware) and `dotfmt/languages` (enabled
  languages for the folder).

```sh
dotfmt-lsp            # stdio; VS Code extension starts it for you
cargo test -p dotfmt-lsp
```
