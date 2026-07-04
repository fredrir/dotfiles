# Shared .zshrc loader (used on every machine).
#
# Fragments live in ~/.config/zsh/conf.d/ and are stowed there from several
# packages: shared/zsh (cross-platform), plus the per-OS package for this host
# (linux/common/zsh or macos/zsh). They are numbered so load order is
# deterministic regardless of which package provided them.
#
# Load order:
#   1. oh-my-zsh bootstrap (05-ohmyzsh) — sets $ZSH, theme, plugins; MUST run
#      before oh-my-zsh.sh.
#   2. oh-my-zsh itself.
#   3. every conf.d fragment (the bootstrap is re-sourced harmlessly).
ZCONF="$HOME/.config/zsh/conf.d"

[[ -f "$ZCONF/05-ohmyzsh.zsh" ]] && source "$ZCONF/05-ohmyzsh.zsh"
[[ -f "$ZSH/oh-my-zsh.sh" ]] && source "$ZSH/oh-my-zsh.sh"

for file in "$ZCONF"/*.zsh; do
  [[ -f "$file" ]] && source "$file"
done

# Rust/cargo (or uv) env, if installed.
[[ -f "$HOME/.local/bin/env" ]] && source "$HOME/.local/bin/env"

# opencode
export PATH=/home/fredrir/.opencode/bin:$PATH
