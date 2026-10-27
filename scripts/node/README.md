# `scripts/node`

Thin Node to Rust bridge Workspace. Current packages:

## dot-code

Behaviour
- Extract: put the cursor anywhere in {a:string,b:string}, or on the destructured {a,b}, and it becomes interface Props {…} above the function, placed before any JSDoc on it. Inline rename starts on Props right away.
- What extract carries over: type parameters the type uses come along (Props<T>), and a type that uses a local declaration stays inside that scope. If Props is taken it becomes Props2. Comments and multi-line formatting are kept, and indentation, semicolons and line endings follow the file.
- Convert: type X = A & {…} ↔ interface X extends A {…}.
- When convert is refused: for unions, merged interfaces, declare global blocks or default exports it doesn't convert, and pressing the key shows the reason instead.
- Menus: both refactors also appear in the lightbulb and the Refactor menu, like the built-in TypeScript refactors.
- Settings: dotCode.typescript.extract.name (supports ${name}, e.g. ${name}Props gives TestProps), .export (never / always / inherit), .rename, and dotCode.server.path.

Architecture
- scripts/rust/crates/dot-code/ (Rust workspace members):
  - core: the Feature trait plus the Action and Edit types. It knows nothing about LSP.
  - typescript: the refactors, built on oxc. The file is parsed once per request.
  - server: the LSP binary. It handles incremental document sync, UTF-16 positions and settings.
- scripts/node/packages/dot-code: about 80 lines of TS that start the server, pass settings through, and trigger the rename.
- Adding a feature: implement Feature, add it to features() in server/src/lib.rs, and add its language to LANGUAGES in extension.ts if it's a new one.

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