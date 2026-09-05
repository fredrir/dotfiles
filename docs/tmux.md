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

Copy mode: `v` character selection, `V` line, Ctrl-v rectangle, `/` search,
`[`/`]` prompt boundaries, `{`/`}` command-output boundaries, Esc exit.
The action palette's **Read actual input bytes** shows bytes delivered past
WezTerm and tmux; keys consumed by an outer layer cannot appear there.

## Surfaces and state

| Surface | Lifetime / behavior |
| --- | --- |
| Scratch | One retained shell per project; hide/reopen preserves processes and variables |
| Native float | tmux 3.7+; focus other panes without dismissing scratch |
| Shelf | Parked panes keep running on this host, including when detached |
| Pickers | Client-local fzf interfaces; plain searchable selection if fzf is absent |
| Status | Actual execution hostname; session and per-client origin when space permits |
| State indicators | Copy, zoom, sync, prefix/resize/nested mode and plugin installation failure |
| Recovery | Server-wide structure and supported programs; not process memory or active turns |

`~/.config/tmux/favorites` accepts one project path per line. The project chooser
also reads zoxide and scans `~/dotfiles`, `~/projects`, `~/sndbx`, `~/llunde-new`.
Git worktrees are discovered on demand. No completion inbox or background project
indexer runs.

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
dotfile sync                    # normal dotfile linking/build pipeline
tmux-workspace doctor
tmux-workspace reload           # validates includes on a disposable server first
~/.config/tmux/bin/tmux-plugins status
~/.config/tmux/bin/tmux-plugins install  # explicit retry, from inside tmux
```

| Requirement | Source / behavior |
| --- | --- |
| tmux 3.3+, Python 3, Bash, SSH, Git | Shared requirements; tmux 3.7+ recommended |
| fzf, zoxide, Lazygit, Yazi | Arch package list and macOS Brewfile |
| smart-splits.nvim | Pinned lazy.nvim plugin; pane metadata avoids process inspection on movement |
| tmux-fingers 2.7.1 | SHA-256-verified native Linux x86_64 / macOS arm64 release assets |
| tmux-resurrect | Exact Git revision in `shared/tmux/plugins.lock.json` |
| Startup installation | Background worker, serialized downloads, five-minute retry backoff |
| Offline startup | `DOTFILES_TMUX_OFFLINE=1`; installed plugins remain available |
| Plugin directory | `${XDG_DATA_HOME:-~/.local/share}/tmux/plugins` |
| Recovery directory | `${XDG_STATE_HOME:-~/.local/state}/tmux/resurrect/<socket-id>` |

Quick-select falls back to client-local fzf in floating panes or with multiple
clients: upstream fingers temporarily changes global input state. Recovery
restore requires at most one attached client and preserves existing panes.
Neither feature claims to restore arbitrary running processes.

| tmux version | Additional capability |
| --- | --- |
| 3.3 | Core workspace, clipboard writes and popup scratch fallback |
| 3.4 | OSC 133 prompt/output navigation and themed native menus |
| 3.5 | Explicit CSI-u extended-key output |
| 3.7 | Non-modal floating panes, clipboard reads and hybrid copy-mode line numbers |

## Design and checks

Colors come from the existing theme generator, including picker colors and
contrast-checked indexed hint colors. Change the theme through `dotfile theme`,
then reload tmux; do not edit generated `shared/tmux/theme.conf`.

```sh
python3 -m unittest discover -s shared/tmux/tests -v
cargo test --manifest-path scripts/rust/Cargo.toml -p agent-hop
```

[fzf](https://github.com/junegunn/fzf) provides selection; the small controller
keeps shelf, scratch, host metadata and handoff in one place.
[sesh](https://github.com/joshmedeski/sesh) was evaluated but not added: its
session layer would duplicate this controller without replacing those features.
Upstream references: [tmux](https://github.com/tmux/tmux/wiki/Getting-Started),
[fingers](https://github.com/Morantron/tmux-fingers),
[smart-splits](https://github.com/mrjones2014/smart-splits.nvim#tmux),
[resurrect](https://github.com/tmux-plugins/tmux-resurrect).
