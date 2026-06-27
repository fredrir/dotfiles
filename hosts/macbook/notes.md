# macbook (template)

macOS machine. Rename this directory to match `hostname -s` so
`./setup.sh` (no args) picks it up automatically.

- Terminal: wezterm (stowed to `~/.config/wezterm`). iTerm assets are reference
  only — see `macos/iterm/`.
- Run `brew bundle --file macos/Brewfile` once you populate the Brewfile.

Install: `./setup.sh macbook`  (or `./setup.sh` after renaming this dir)
