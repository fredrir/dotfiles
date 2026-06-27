# Linux-only environment (loaded after the shared 10-env fragment).

# gnome-keyring SSH agent socket.
export SSH_AUTH_SOCK="$XDG_RUNTIME_DIR/keyring/ssh"

# Hyprland config dir (used by the cdhy alias and scripts). $CONFIG is set in
# the shared 10-env fragment.
export HYPR="$CONFIG/hypr"

# Global npm prefix.
export PATH="$HOME/.local/share/npm-global/bin:$PATH"
