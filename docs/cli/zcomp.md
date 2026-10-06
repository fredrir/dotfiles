# zcomp

## Commands

<!-- cli:commands:start -->
| Command          | Description                                                                         |
| ---------------- | ----------------------------------------------------------------------------------- |
| `zcomp`          | Prints zsh completion candidates for npm, pnpm, yarn, bun, their runners, and pi.   |
| `zcomp complete` | Prints the candidates for the word under the cursor; called by the zsh integration. |
| `zcomp refresh`  | Rebuilds one cached source; started in the background when a cache goes stale.      |
| `zcomp warm`     | Starts background rebuilds for missing or stale caches; run at shell start.         |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                             |
| ----------------------- | --------------------------------------------------------------------------------------- |
| `--command <COMMAND>`   | Selects the command being completed, such as `pnpm` or `bunx`.                          |
| `--current <INDEX>`     | Selects the cursor word by its position, counting from 1 like `$CURRENT`.               |
| `--prefix <PREFIX>`     | Sets the cursor word up to the cursor, like `$PREFIX`.                                  |
| `--color`               | Colors the candidates with the dotfile theme; set by the zsh integration under fzf-tab. |
| `-h`, `--help`          | Shows help for the selected command and exits.                                          |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                         |
| `-V`, `--version`       | Prints the version and exits.                                                           |
<!-- cli:flags:end -->

## Commands completed

| Command | Source |
| --- | --- |
| `npm`, `pnpm`, `pn`, `yarn`, `bun` | Their `--help`, `package.json`, registry search |
| `npx`, `pnpx`, `pnx`, `bunx` | `node_modules/.bin`, then as `add` |
| `pi` | `pi --help`, `pi --list-models`, sessions, settings |

## Env

| Env                           | Default                                    |
| ----------------------------- | ------------------------------------------ |
| `ZCOMP_CACHE_DIR`             | `$XDG_CACHE_HOME/zcomp` (`~/.cache/zcomp`) |
| `ZCOMP_OFFLINE`               | unset; `1` skips the network               |
| `ZCOMP_FOREGROUND`            | unset; `1` rebuilds stale caches inline    |
| `PI_CODING_AGENT_DIR`         | `~/.pi/agent`                              |
| `PI_CODING_AGENT_SESSION_DIR` | settings `sessionDir`, then per-cwd dir    |

## Search

| Name    | Value                                                       |
| ------- | ----------------------------------------------------------- |
| Sources | npm registry search, npms.io prefix suggestions             |
| Match   | Name contains the typed word; the pick replaces the word    |
| Order   | Monthly downloads, highest first                            |
| Offline | Earlier answers for a shorter word, then `bun getcompletes` |
| Bare    | Popular commands (`-g`, runners), popular libraries (local) |

## Colors

| Part                            | Theme role                       |
| ------------------------------- | -------------------------------- |
| Name                            | `accent`                         |
| Package scope (`@scope`)        | `info`                           |
| Scope marker (`@`)              | `ansi.bright.green`              |
| Scope separator (`/`)           | `muted`                          |
| Option, provider, source type   | `info`                           |
| Downloads, provider column      | `info`                           |
| Description, range, age         | `muted`                          |
| `default`, dist-tag             | `success`                        |
| Dependency kind (`dev`, `peer`) | `warning`                        |
| Enabled                         | fzf-tab loaded; `NO_COLOR` unset |

## Cache

| Source                  | Refreshed                                     |
| ----------------------- | --------------------------------------------- |
| Manager and pi `--help` | Binary or pi settings change                  |
| pi models               | 24h, or pi, `models.json`, `auth.json` change |
| pi package gallery      | 24h                                           |
| Popular packages        | 24h, in the background                        |
| Registry search         | 1h per word                                   |
| Package versions        | 1h                                            |