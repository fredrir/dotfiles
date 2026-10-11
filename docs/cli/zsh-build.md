# zsh-build

## Commands

<!-- cli:commands:start -->
| Command           | Description                                                                          |
| ----------------- | ------------------------------------------------------------------------------------ |
| `zsh-build`       | Compiles the zsh startup config into one bundle with fixed command output folded in. |
| `zsh-build where` | Prints the file and line where bundled shell functions were defined.                 |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                             |
| ----------------------- | ----------------------------------------------------------------------- |
| `-n`, `--dry-run`       | Shows whether the bundle would change without writing it.               |
| `--root <ROOT>`         | Sets the dotfiles checkout; defaults to `$DOTFILES`, then `~/dotfiles`. |
| `-h`, `--help`          | Shows help for the selected command and exits.                          |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.         |
| `-V`, `--version`       | Prints the version and exits.                                           |
<!-- cli:flags:end -->

## Files

| Path | Value |
| --- | --- |
| `config/zsh/build.toml` | Targets, ambient variables, commands folded at build time, deferred code |
| `.cache/zsh/build/zshrc.zsh` | Bundle; built by `dotfile sync` |
| `.cache/zsh/build/zshrc.zsh.zwc` | Wordcode; compiled by the build with `$SHELL` if zsh, else `zsh` on `PATH`. A shell recompiles it in the background when older than the bundle or from another zsh version |
| `.cache/zsh/build/zshrc.origins` | Function name to defining `file:line`; read by `zsh-build where` and the bundle's `type`/`whence` |
| `.cache/zsh/build/zshrc.completions/` | `cached_eval` output that is an autoloadable `#compdef` script; first in `fpath` at a deferred `compinit`, loaded on the first Tab |
| `.cache/zsh/build/commands.json` | `[fold] commands` and `cached_eval` output kept between builds; reused until the program (symlinks resolved, mtime, size) or its `PATH` changes; a `#!` script also until its git checkout's commit changes, and outside one is not kept; zsh's check of respelled generated code, until zsh changes |
| `~/.zcompdump-*.fpath` | `fpath` the completion dump was built for; while it matches and the dump is not newer, the bundle runs `compinit -C`. A build removes a dump whose completion functions changed |
| `.cache/zsh/build/zshrc.rcs.zsh` | Guard sourced by `~/.zshenv`; only when every global startup file compiled |
| `shared/zsh/00-profile.zsh` | `~/.zprofile` (macOS) |

## Target

| Key | Value |
| --- | --- |
| `profile` | Repo file linked as `~/.zprofile`; compiled into the login block |
| `system` | `true`: compiles the global `zprofile` and `zshrc` into this target; one target max |

## System

| Key | Default |
| --- | --- |
| `dir` | `/etc` on macOS, `/etc/zsh` elsewhere |
| `ambient` | `[]`; variables known while global files run (`PATH` is never known) |
| `path_helper_root` | `""`; prefix for `/etc/paths`, `/etc/manpaths` (as `PATH_HELPER_ROOT`) |

## Defer

| Key | Default |
| --- | --- |
| `compinit` | `false`; oh-my-zsh's statements from the dump path to `_omz_source` |
| `plugins` | `[]`; oh-my-zsh plugin names, globs |
| `evals` | `[]`; `cached_eval` names, globs |
| `files` | `[]`; sourced files relative to the root, globs |

| Runtime | Value |
| --- | --- |
| Prompt follows | Queued through `defer` at its original place; runs after the first prompt |
| `zsh -i -c`, `zsh -i script`, no zle | Runs in place |
| Line typed before the queue empties | Queue runs first |
| `compdef` before a deferred `compinit` | Queued in order with the deferred code |
| Deferred `compinit` | Sees `fpath` as of its original place, `zshrc.completions/` first |
| `cached_eval` of a `#compdef` script | File in `zshrc.completions/`; inlined without a deferred `compinit` |

| Kept in place when | Value |
| --- | --- |
| Top level | `typeset`, `local`, `trap`, `emulate -L`, local options, `return`, loop control |
| compinit section | `$0`; sets `fpath`; shares lines |
| Later startup code | Changes or calls an alias, function, widget, zstyle, option, variable, completion or key binding it sets; changes a key binding it reads |
| Any unit | Adds a precmd hook; `defer` unrecognized |

## Global startup files

| Shell | Global rcs |
| --- | --- |
| Interactive, guard current | Off; bundle runs `zprofile` (login), `~/.zprofile` (login), `zshrc` |
| Non-interactive, `ZSH_BUILD_SKIP=1`, guard stale | On; zsh reads them |

| Guard stale when | Value |
| --- | --- |
| Newer than guard | Global dir, compiled files, `~/.zprofile` target, `path_helper` data |
| `~/.zprofile` | Not the linked `profile` |

| Left to zsh | Value |
| --- | --- |
| `emulate` | Not modeled |
| Unrewritable `return`, parse error | Same as inlined files |
| `~/.zprofile` | Not linked to `profile` |
| `zlogin`, `zlogout` | Always read by zsh |

| Native | Value |
| --- | --- |
| `eval` of `path_helper -s` | Native zsh; runs `path_helper` when `PATH`/`MANPATH` hold `$`, `` ` ``, `\`, `"`, tabs, newlines or double spaces |
| `eval "$(cmd)"` | Output inlined when `cmd` is in `[fold] commands` |
| `eval "$(cmd)"` in a function | Output inlined as a `{ }` block when it parses and reads no call frames or line numbers |
| `((( a )) && b)` in output | Respelled `( (( a )) && b)` when zsh reads both alike |
| `$(atuin uuid)` | Native UUIDv7; runs `atuin uuid` when `/dev/urandom` or `epochtime` fail; kept when its output changes form |
| `$(locale LC_CTYPE)` | `${langinfo[CODESET]}`; runs `locale` when `zsh/langinfo` fails; kept when `locale` prints more than the codeset |
| `$(emulate)` | `zsh` at top level; kept once compiled code may change the emulation |
| `$(command -v name)` | Path on the build search path; kept for functions, aliases, builtins, function bodies, loops |

| Decided | Value |
| --- | --- |
| `if` chain | Only the arm that runs; `:` when none does |
| `test && cmd`, `test \|\| cmd` | `cmd`; `:` or `! :` (same status) when it never runs |
| Tests | `has_cmd name`, `(( $+commands[name] ))`, `true`, `false`, `[[ ]]` on constants; with `!`, `&&`, `\|\|` |
| Function run once | Tests in its body, when only definitions lie between it, the statement it starts and its `unset -f` or `unfunction`, and its file names it nowhere else |
| Kept | Variable and file tests, loop bodies, arms sharing lines or reading `$?` first |

## Switches

| Name                | Value                                          |
| --- | --- |
| `ZSH_BUILD_SKIP=1`  | Loads the `shared/zsh` sources and lets zsh read the global startup files |
| `# zsh-build: omit` | Drops the statement that follows from a bundle |