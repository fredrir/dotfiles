# `scripts/node`

Thin Node to Rust bridge Workspace. Current packages:

## dot-code

Behaviour

- Extract: put the cursor anywhere in {a:string,b:string}, or on the destructured {a,b}, and it becomes interface Props {…} above the function, placed before any JSDoc on it. Inline rename starts on Props right away.
- What extract carries over: type parameters the type uses come along (Props<T>), and a type that uses a local declaration stays inside that scope. If Props is taken it becomes Props2. Comments and multi-line formatting are kept, and indentation, semicolons and line endings follow the file.
- Convert: type X = A & {…} ↔ interface X extends A {…}.
- When convert is refused: for unions, merged interfaces, declare global blocks or default exports it doesn't convert, and pressing the key shows the reason instead.
- Menus: both refactors also appear in the lightbulb and the Refactor menu, like the built-in TypeScript refactors.
- Keys: shift+alt+x extracts an interface, shift+alt+cmd+x a type alias, shift+alt+cmd+t converts. All three are in the editor right-click menu and under Dot Code in Keyboard Shortcuts.
- Settings: dotCode.typescript.extract.name (supports ${name}, e.g. ${name}Props gives TestProps), .export (never / always / inherit), .rename, and dotCode.server.path.

Architecture

- scripts/rust/crates/dot-code/ (Rust workspace members):
  - core: the Feature trait plus the Action and Edit types. It knows nothing about LSP.
  - typescript: the refactors, built on oxc. The file is parsed once per request.
  - workspace: next/previous file in Explorer order (ICU4X collation, files.exclude) and package manager detection.
  - server: the LSP binary. It handles incremental document sync, UTF-16 positions, settings and the dotCode/siblingFile and dotCode/packageManager requests.
- scripts/node/packages/dot-code: thin TS client that starts the server, restarts it after crashes, passes settings through, and triggers the rename. The language client is a separate bundle (dist/client.js) loaded right after activation.
- Adding a feature: implement Feature, add it to features() in server/src/lib.rs, and add its language to LANGUAGES in src/lib/constants.ts if it's a new one.

Build and install

```sh
bun run deploy builds both binaries                                            # (about 23 s, 1.8 MB vsix)
```

```sh
bun run deploy -- --remote                                                      # Archie also installs it on Archie.
```

```
bun run build -- --host                                                          # Builds only for the current machine.
```

- **Cross-compiling:** Rust's own linker produces the static Linux binary, so no zig is needed.
- **Script name:** it's deploy rather than install, because a script named install would run on every bun install.