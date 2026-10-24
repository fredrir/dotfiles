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
| `.cache/zsh/build/zshrc.zsh` | Bundle; built by `dotfile sync`, which runs `zsh-build verify` after a change and warns per difference |
| `.cache/zsh/build/zshrc.zsh.zwc` | Wordcode; compiled by the build with `$SHELL` if zsh, else `zsh` on `PATH`. A shell that read the bundle as text (`$ZSH_EVAL_CONTEXT` not ending in `filecode`), as zsh does when the wordcode is older or from another zsh version, recompiles it in the background; otherwise no process starts |
| `.cache/zsh/build/zshrc.origins` | Function name to defining `file:line`; read by `zsh-build where` and the bundle's `type`/`whence` |
| `.cache/zsh/build/zshrc.completions/` | `cached_eval` output that is an autoloadable `#compdef` script; first in `fpath` at a deferred `compinit`, loaded on the first Tab |
| `.cache/zsh/build/commands.json` | `[fold] commands` and `cached_eval` output kept between builds; reused until the program (symlinks resolved, mtime, size) or its `PATH` changes; a `#!` script also until its git checkout's commit changes, and outside one is not kept; zsh's check of respelled generated code, until zsh changes; completion files plugin jobs write, until the command changes; zsh-autosuggestions' recorded binding, until the bundle changes |
| `$ZSH_CACHE_DIR/completions/_NAME` | Completion file an oh-my-zsh plugin's background job rewrites on each start; written by the build |
| `~/.zcompdump-*.fpath` | `fpath` the completion dump was built for; while it matches and the dump is not newer, the bundle runs `compinit -C`. A build removes a dump whose completion functions changed, and `compdump`'s staging files (`<dump>.$HOST.<pid>`) older than a minute |
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
| `defer source FILE` | `FILE` compiled with the state startup leaves, as a function `defer` queues in its place; as written when it does not compile; top level only |
| Deferred `compinit` | Sees `fpath` as of its original place, `zshrc.completions/` first |
| `cached_eval` of a `#compdef` script | File in `zshrc.completions/`; inlined without a deferred `compinit` |

| Kept in place when | Value |
| --- | --- |
| Top level | `typeset`, `local`, `trap`, `emulate -L`, local options, `return`, loop control |
| compinit section | `$0`; sets `fpath`; shares lines |
| Later startup code | Changes or calls an alias, function, widget, zstyle, option, variable, completion or key binding it sets; changes a key binding or zstyle it reads |
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

| Their aliases | Value |
| --- | --- |
| `zshenv`, `zprofile`, `~/.zprofile`, `zshrc` | Expanded in the code after them, compiled or left to zsh; `system = true` target only |
| `zprofile`, `~/.zprofile` | Login shells only, so not known on every path |
| File that does not parse | Any alias may be set |

| Native | Value |
| --- | --- |
| `eval` of `path_helper -s` | Native zsh; runs `path_helper` when `PATH`/`MANPATH` hold `$`, `` ` ``, `\`, `"`, tabs, newlines or double spaces |
| `eval "$(cmd)"` | Output inlined when `cmd` is in `[fold] commands` |
| `eval "$(cmd)"` in a function | Output inlined as a `{ } always` block when it parses and reads no call frames or line numbers |
| `((( a )) && b)` in output | Respelled `( (( a )) && b)` when zsh reads both alike |
| `$(atuin uuid)` | Native UUIDv7; runs `atuin uuid` when `/dev/urandom` or `epochtime` fail; kept when its output changes form |
| `$(locale LC_CTYPE)` | `${langinfo[CODESET]}`; runs `locale` when `zsh/langinfo` fails; kept when `locale` prints more than the codeset |
| `$(emulate)` | `zsh` at top level; kept once compiled code may change the emulation |
| `$(command -v name)` | Path on the build search path; kept for functions, aliases, builtins, function bodies, loops |
| `command rm -f FILE...` | Runs `rm` only when a file is there; `FILE` plain, or double-quoted with only `$`, `{`, `}` expansions |
| Background job writing `"$ZSH_CACHE_DIR/completions/_NAME"` | Run once by the build against a scratch cache, `NAME` on the search path; its file written by the build; the bundle runs the job only when the file is missing. Kept when it reads a zstyle set before it |

| Decided | Value |
| --- | --- |
| `if` chain | Only the arm that runs; `:` when none does |
| `test && cmd`, `test \|\| cmd` | `cmd`; `:` or `! :` (same status) when it never runs |
| Tests | `has_cmd name`, `(( $+commands[name] ))`, `true`, `false`, `[[ ]]` on constants; with `!`, `&&`, `\|\|` |
| Function run once | Tests in its body, when only definitions lie between it, the statement it starts and its `unset -f` or `unfunction`, and its file names it nowhere else |
| Kept | Variable and file tests, loop bodies, arms sharing lines or reading `$?` first |

## zsh-autosuggestions

| Key | Value |
| --- | --- |
| Recorded | First run of `_zsh_autosuggest_bind_widgets`, in a login shell on a pty started from the bundle, after `defer source` compiled the plugin; once per bundle text |
| Emitted | After the plugin: the functions and widgets that run made, and `_ZSH_AUTOSUGGEST_BIND_COUNTS`, as code; no `eval` |
| Runs when | Widgets and their values, and `ZSH_AUTOSUGGEST_*` parameters, are as recorded; otherwise the plugin binds them |
| After | The plugin's `_zsh_autosuggest_bind_widgets` is back; the build's helpers are gone |
| Left to the plugin | Run changes other parameters, removes a function, widget or parameter, or makes a widget other than `user:` or `completion:`; names a function that is not plain; code names an alias set before the bundle; dry run without a record |

## Errors

| Error in | Value |
| --- | --- |
| Inlined file, `cached_eval`, `_omz_source` | Stops that file, as `source` does; in a `{ } always` block that records it for `_zsh_build_status` after it |
| Global startup file | Stops that file, as zsh does; in a `{ } always { TRY_BLOCK_ERROR=0 }` block |
| `eval "$(cmd)"` output, in a function too | Stops that output, as `eval` does; same block |
| `$?` after | 126 for a file, 1 for `eval` output, as zsh gives |
| `$0` | Restored in the `always` block |
| Statement parsed when it runs | Stops its file, as in the sources: `_zsh_build_raise` raises the error `eval` cleared |

## Aliases

| Expanded | Value |
| --- | --- |
| Where | As zsh parses the sources: a sourced file one statement at a time, `eval` output at once, a function body with its definition; not in `$(...)` |
| Which | Regular in command position, and the next word after a value ending in a space; global anywhere; suffix on a command word; never quoted or escaped words, nor the alias being expanded |
| Aliases | Set before in source order on every path, deferred code's included; a `return` from a file or function is a path; `aliases` option honored |
| Bundle | Code in one block, which zsh parses in one piece, as text and as wordcode, before it sets any alias |
| Spelled | A word an alias set before the bundle could expand again is quoted, as `\grep --color=auto`; `.zshenv` and global startup files |
| Checked | zsh reads the statement with its aliases as the expansion without them, and the quoted text the same either way; the parser reads the same shape |

| Parsed when it runs | Value |
| --- | --- |
| Statement | Uses an alias `not known on every path`, the `aliases` option likewise, or such an alias names a function |
| Emitted | `\builtin eval '{ ... } always { ... }'` of its text, then `_zsh_build_raise`: zsh expands with the aliases in effect, on whichever path ran |
| Only when | Its own statement in a sourced file, outside deferred code and loops, with no other rewrite, `$0` or file-level `return`; a function definition, or code reading no `funcstack`, `zsh_eval_context`, `LINENO`, `%N` and the like |
| Deferred code | Setting or removing an alias named by a word of it: kept in place, `alias NAME changed later` |
| `type`, `whence`, `where` | Report the original `file:line` |

| Left as written, with a warning | Value |
| --- | --- |
| `not known on every path` | As above, where it cannot be parsed when it runs |
| `value is not one command of plain words` | Operators, redirections, assignments or reserved words in the value |
| `names a function` | `name()` where `name` may be an alias; as above |
| `set by code the build does not follow` | Any alias may be set |
| `aliases option not known on every path` | `setopt aliases` or `no_aliases` on some paths only; as above |
| `statement not read by the build` | Syntax the word reader does not follow, and zsh's check fails |
| `zsh reads the expansion otherwise` | The check fails |
| `zsh not found` | Nothing to check with |

## Verify

| Run | Value |
| --- | --- |
| Shells | `-zsh` on a pty, bundle then sources (`ZSH_BUILD_SKIP=1`); never rebuilds |
| `--direct` | The bundle shell sources the `--root` target's bundle in place of `~/.zshrc` |
| `ZDOTDIR`, cwd | Scratch dir: links to `~/.zshenv`, `~/.zprofile`, `~/.zlogin`, `~/.zlogout`; `.zshrc` sources `~/.zshrc`, then the dump hook |
| Environment | Caller's terminal variables (`TERM`, `LANG`, `LC_*`, `WEZTERM_*`, ...) and `ambient`; `PATH=/usr/bin:/bin:/usr/sbin:/sbin`; no `WEZTERM_GO_TO`, `ZSH_COMPDUMP` |
| Dump | At the first prompt once `_defer_queue` is empty; then `kill -KILL` |
| Compared | Options, aliases (all kinds), functions, widgets, key bindings per keymap, keymaps, zstyles, named dirs, modules, disabled names, math functions, traps, global parameters and their types |
| Not written | History (`HISTFILE` scratch after `~/.zshrc`), bundle, wordcode, `~/.zcompdump*`, zoxide data (`_ZO_DATA_DIR` scratch); startup's own cache refreshes still run |

| Stops | Value |
| --- | --- |
| `.zwc` not current | The bundle would recompile it |
| Bundle | Not built; not loaded by `~/.zshrc`; loaded with `ZSH_BUILD_SKIP=1` |
| Stale bundle | `<bundle>: stale`; compared as built |

| Tolerated | Value |
| --- | --- |
| zsh-build's own | `_zsh_build_*`, `__zb<N>`, `__zb0_<N>`, `__zb_head`, `__zbv_*`; the bundle's `type`, `whence`, which run the builtins when copied without `_zsh_build_whence` |
| `ZSH_BUILD_SKIP` | Sources only |
| `fpath` | Bundle's `<target>.completions` dropped |
| `<target>.completions/` scripts | Sourced in the bundle shell before the dump, aliases off, as the first Tab loads them |
| Function bodies | As zsh lists them; `${__zbN}` for a `$(cmd)`, a `{ } always` block for `eval "$(cmd)"`, native expansions for their command; a backslash before a word, which the build adds against a second alias expansion |
| `add-zle-hook-widget` indices | Order only |
| Queue residue, bundle only | `add-zle-hook-widget`, `azhw:*`, `_defer_drain`, `_defer_typeahead`, `zle-*` dispatchers, `zle-hook types` |
| `menu-select` | Completion widget in the bundle, complist's builtin in the sources: deferred `compinit` runs after `zmodload zsh/complist`; with zsh-autosuggestions' copy |
| Session values | `ATUIN_SESSION`; `$$`, `RANDOM`, `SECONDS`, `TTY`, `COLUMNS`, ... not dumped |

## Switches

| Name | Value |
| --- | --- |
| `ZSH_BUILD_SKIP=1` | Loads the `shared/zsh` sources and lets zsh read the global startup files |
| `# zsh-build: omit` | Drops the statement that follows from a bundle |