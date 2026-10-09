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
| `.cache/zsh/build/zshrc.zsh.zwc` | Wordcode; compiled in the background by the first shell after a build |
| `.cache/zsh/build/zshrc.origins` | Function name to defining `file:line`; read by `zsh-build where` and the bundle's `type`/`whence` |
| `~/.zcompdump-*.fpath` | `fpath` the completion dump was built for; while it matches, the bundle runs `compinit -C`. A build removes a dump whose completion functions changed |
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
| Deferred `compinit` | Sees `fpath` as of its original place |

| Kept in place when | Value |
| --- | --- |
| Top level | `typeset`, `local`, `trap`, `emulate -L`, local options, `return`, loop control |
| compinit section | `$0`; sets `fpath`; shares lines |
| Later startup code | Changes or calls an alias, function, widget, zstyle, option, variable or completion it sets |
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
| `$(atuin uuid)` | Native UUIDv7; runs `atuin uuid` when `/dev/urandom` or `epochtime` fail; kept when its output changes form |

## Switches

| Name                | Value                                          |
| --- | --- |
| `ZSH_BUILD_SKIP=1`  | Loads the `shared/zsh` sources and lets zsh read the global startup files |
| `# zsh-build: omit` | Drops the statement that follows from a bundle |