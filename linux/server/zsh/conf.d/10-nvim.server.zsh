# Headless server: run Neovim in minimal mode (see shared/nvim init.lua) — no
# LSP / mason / formatters / linters / AI / DB / debug, so nothing pulls node,
# Go or a language-server toolchain. Editor + treesitter + telescope stay.
export NVIM_MINIMAL=1

# nvim as the default editor (git commits, `EDITOR`-driven tools, ...).
export EDITOR=nvim
export VISUAL=nvim
