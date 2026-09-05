# Terminal workspace

```sh
t                         # enter this directory's project workspace
t ~/projects/example      # open another project
t --session research      # named workspace
tmux-workspace doctor     # versions, tools and plugin health
```

## Ownership

| Layer | Owns |
| --- | --- |
| WezTerm | Desktop windows, fonts, OS clipboard and physical keyboard translation |
| tmux | Project sessions, windows, panes, scratch, scrollback and remote attachment |
| Shell / Neovim | Editing, command history and editor splits |
| agent-hop | Explicitly managed agent execution and cross-machine handoff |

WezTerm's familiar shortcuts target tmux while its attachment marker is present;
outside tmux they retain their terminal actions. `Primary` means Cmd on macie,
Ctrl on archie. On Linux, Ctrl-C remains interrupt and Ctrl-V remains shell input;
clipboard copy/paste use Ctrl-Shift-C/V.

## Keys

`P` means press Ctrl-b, release, then press the key. `P ?` searches the running
server's bindings; `P Space` searches and executes actions.

| Key | Action |
| --- | --- |
| `P d`, `P '` | Split right / below, retaining the working directory |
| `P h/j/k/l` | Move through Neovim splits and tmux panes |
| `P ;`, `P e` | Previous pane / numbered pane chooser |
| `P z`, `P m`, `P M` | Zoom / choose a pane to promote / promote current pane |
| `P t`, `P n/p`, `P Tab` | New / next or previous / last active window |
| `P 1` … `P 9`, `P 0` | Numbered window / last numbered window |
| `P q`, `P w` | Close pane / window, protecting running jobs |
| `P R` | Resize mode: hjkl, Shift for larger steps, `=` balance, Esc finish |
| `P Y` | Toggle synchronized input; status stays visibly marked |
| `P s` | Projects, Git worktrees, favorites and running sessions |
| `P S`, `P .` | Park a live pane / retrieve it from the shelf |
| ``P ` `` or `P *` | Show / hide persistent scratch |
| `P g`, `P y` | Contextual Lazygit / Yazi |
| `P f` | Hint letters copy; Shift+hint pastes; Enter copies in fzf fallback |
| `P Enter`, `P [` | Copy mode; vi selection, `y` copies |
| `P Up/Down`, `P O` | Previous / next prompt; search deep output |
| `P I` | Routing, terminal capabilities and key-table inspector |
| `P F7` | Connect to another host without closing the source pane |
| `P a`, `P A` | Agent conversation picker / managed execution handoff |
| `P Ctrl-s`, `P Ctrl-r` | Explicit recovery save / restore |
| `P r`, `P D` | Validate and reload / detach this client |
| `P Ctrl-b` | Send literal Ctrl-b to the application |
| `P B` | Nested forwarding; Ctrl-b Esc returns to outer tmux |

| WezTerm gesture | tmux action |
| --- | --- |
| Primary-Space / Primary-Shift-P | Actions / projects |
| Primary-`;` | Scratch |
| Primary-Shift-Space | Quick-select |
| Primary-Shift-X | Copy mode |
| Primary-Up/Down / Primary-Shift-Up | Prompt motion / output search |
| Primary-F12 | Show which layer owns shortcuts |
| Primary-Shift-F12 | Manually toggle routing for an unusual nested attachment |

| Copy-mode input / indicator | Action / value |
| --- | --- |
| Mouse drag / double-click / triple-click | Copy selection / word / line; keep highlight and scroll position |
| `v`, `V`, Ctrl-v | Character / line / rectangular selection |
| `y`, Enter | Copy selection and exit |
| Esc, `q` | Exit |
| `/`, `?` | Search forward / backward |
| `g`, `G` | Oldest retained history / bottom |
| `[`, `]` / `{`, `}` | Previous / next prompt / command-output boundary |
| History limit | 100,000 lines per pane |
| Copy-position indicator | Scroll offset / retained lines; `limit` shows capacity |

The action palette's **Read actual input bytes** shows bytes delivered past
WezTerm and tmux; keys consumed by an outer layer cannot appear there.

## Surfaces and state

| Surface | Lifetime / behavior |
| --- | --- |
| Scratch | One retained shell per project; hide/reopen preserves processes and variables |
| Native float | Focus other panes without dismissing scratch |
| Shelf | Parked panes keep running on this host, including when detached |
| Pickers | Client-local fzf interfaces; numbered selection if fzf is absent |
| Status | Actual execution hostname; session and per-client origin when space permits |
| State indicators | Copy, zoom, sync, prefix/resize/nested mode and server-local plugin failure |
| Recovery | Server-wide structure and supported programs; not process memory or active turns |

| Project input | Value |
| --- | --- |
| `${XDG_CONFIG_HOME:-~/.config}/tmux/workspace.toml` | Explicit paths, search roots, zoxide/worktree switches and result limit |
| Default paths / roots | `~/dotfiles` / `~/projects`, `~/sndbx`, `~/llunde-new` |
| `${XDG_CONFIG_HOME:-~/.config}/tmux/favorites` | One project path per line; `tmux-workspace favorite` appends once |
| Git worktrees | On-demand, bounded parallel discovery; no background indexer |
| Host choices | `config/hosts.dotfile`; `DOTFILES_HOSTS_FILE` overrides discovery |

## Host switching and execution

Inside tmux, `archie`, `macie` and `P F7` open SSH-backed destination workspaces.
Outside tmux, the existing WezTerm mutual-TLS path remains available.
An SSH attachment alone does **not** move execution off the source machine.

For managed agent takeover, see [agent-hop](cli/agent-hop.md). Start managed
Codex or Claude through `P Space`; `P A` queues the move. The current turn must
reach its safe boundary before execution ownership transfers. Follow and status
are available in the same action palette.
Codex takeover is native-tested on both hosts. Claude requires destination login
and pretrusted setup; first-use trust prompts safely refuse the move.

## Installation and recovery

```sh
dotfile sync                    # builds, links and provisions pinned plugins
tmux-workspace doctor
tmux-workspace reload           # validates includes on a disposable server first
tmux-workspace plugins status --json
tmux-workspace plugins install  # explicit install/retry; no running server needed
tmux-workspace plugins load     # load installed artifacts into this server
```

| Requirement | Source / behavior |
| --- | --- |
| tmux 3.7c+, Bash, SSH, Git, curl | Runtime requirements; controller is a native Rust binary |
| Cargo, uv, Python 3.12+, pytest, Lua, Zsh | Build and test tools; no Python controller at runtime |
| fzf, zoxide, Lazygit, Yazi | Arch package list and macOS Brewfile |
| smart-splits.nvim | Pinned lazy.nvim plugin; pane metadata avoids process inspection on movement |
| tmux-fingers 2.7.1 | SHA-256-verified native Linux x86_64 / macOS arm64 release assets |
| tmux-resurrect | Exact Git revision in `shared/tmux/plugins.lock.json` |
| Provisioning | `dotfile sync` or `plugins install`; serialized, verified, atomic publication |
| Server startup / reload | Load installed artifacts only; never downloads |
| Offline provisioning | `DOTFILES_TMUX_OFFLINE=1`; reports missing artifacts without downloading |
| Plugin directory | `${XDG_DATA_HOME:-~/.local/share}/tmux/plugins` |
| Install state / load state | `plugins/installation.json` / server-local `@workspace-plugins-state` and `@workspace-plugins-error` |
| Recovery directory | `${XDG_STATE_HOME:-~/.local/state}/tmux/resurrect/<socket-id>` |
| Recovery metadata | Hash-checked `.workspace.json` sidecar; project roots, shelf tags and scratch backing sessions |

Quick-select falls back to client-local fzf in floating panes or with multiple
clients: upstream fingers temporarily changes global input state. Recovery
restore requires at most one attached client and preserves existing panes.
Scratch views are recreated on demand; their retained backing sessions are saved.
Recovery does not restore process memory, network connections or active agent turns.

## Design and checks

| Path | Owns |
| --- | --- |
| `scripts/rust/crates/tmux-workspace/Cargo.toml` | Workspace crate and native binary |
| `scripts/rust/crates/tmux-workspace/src/cli.rs` | Command-specific arguments and generated completions |
| `src/tmux.rs`, `src/process.rs`, `src/config.rs` | Socket/client context, subprocess deadlines, typed settings and state I/O |
| `src/projects.rs`, `src/panes.rs`, `src/clients.rs` | Project discovery, persistent panes and attachment metadata |
| `src/ui.rs`, `src/integrations.rs`, `src/diagnostics.rs` | Pickers, tool adapters, input inspection and reload validation |
| `src/plugins.rs`, `src/recovery.rs` | Pinned provisioning, server loading and recovery |
| `scripts/python/tests/tmux/` | Black-box CLI, isolated tmux servers, real PTYs, plugin and adapter tests |
| `shared/tmux/` | tmux configuration, generated theme, plugin lock and project settings |
| `shared/tmux/bin/` | POSIX forwarding shims; native binary in `~/.local/bin/tmux-workspace` |
| `shared/tmux/libexec/hostname` | Upstream plugin compatibility helper |

Colors come from the existing theme generator, including picker colors and
contrast-checked indexed hint colors. Change the theme through `dotfile theme`,
then reload tmux; do not edit generated `shared/tmux/theme.conf`.

```sh
cargo build --locked --manifest-path scripts/rust/Cargo.toml -p tmux-workspace
cargo clippy --manifest-path scripts/rust/Cargo.toml -p tmux-workspace --all-targets -- -D warnings
uv run --project scripts/python --locked pytest scripts/python/tests/tmux scripts/python/tests/theme/test_tmux.py

# Explicit machine-readable diagnostics and test sockets
tmux-workspace --socket /tmp/workspace-test/socket inspect --json
tmux-workspace projects --json
tmux-workspace panes
```

| Test input | Value |
| --- | --- |
| macOS / Linux | Same pytest command; separate sockets, temporary HOME/XDG directories, no live-server reload |
| `TMUX_BINARY` | Alternate tmux executable; default `tmux` on PATH |
| `TMUX_WORKSPACE_BINARY` | Prebuilt test binary; otherwise the fixture runs Cargo |
| `CARGO_TARGET_DIR` | Alternate Cargo build directory; honored by both native test fixtures |
| `TMUX_RESURRECT_SOURCE` | Pinned plugin checkout for real save/restart/restore tests; defaults to local installed plugin |
| Missing recovery fixture | Explicit skip; run `tmux-workspace plugins install` before the full suite |

[fzf](https://github.com/junegunn/fzf) provides selection; the small controller
keeps shelf, scratch, host metadata and handoff in one place.
[sesh](https://github.com/joshmedeski/sesh) was evaluated but not added: its
session layer would duplicate this controller without replacing those features.
Upstream references: [tmux](https://github.com/tmux/tmux/wiki/Getting-Started),
[fingers](https://github.com/Morantron/tmux-fingers),
[smart-splits](https://github.com/mrjones2014/smart-splits.nvim#tmux),
[resurrect](https://github.com/tmux-plugins/tmux-resurrect).
