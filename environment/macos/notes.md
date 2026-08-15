# macbook / macos

macOS. Stows `shared` + `macos`.

- Terminal: WezTerm, built from fredrir/wezterm at the commit pinned in
  `config/pins.dotfile`. Not a Homebrew cask to ensure identical builds for native mux.
  -  `~/packages/wezterm` (`cargo build` +  `ci/deploy.sh` darwin steps) 
- Run `brew bundle --file macos/Brewfile` once you populate the Brewfile.

Install: `./setup.sh macbook/macos`
