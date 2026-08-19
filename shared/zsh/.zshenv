# Read by every zsh; .zshrc is not. `ssh <host> <cmd>` and the dmux remote agent
# run non-interactively, so ~/.local/bin has to go on PATH here or the commands
# setup installs there do not exist for them at all. `typeset -U` is what makes
# the duplicate prepend in conf.d harmless: the attribute survives into .zshrc.
# Kept minimal — this also runs for every script, and must produce no output.
typeset -U path PATH
path=("$HOME/.local/bin" $path)
export PATH
