local vtabs = require "plugins.vtabs"

return vtabs.action {
  QuickTerminal = { args = { "zsh", "-lc", "exec \"$HOME/.cargo/bin/glyph-search\"" } },
}