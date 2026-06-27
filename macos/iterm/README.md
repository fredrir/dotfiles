# iTerm2 (reference assets — not stowed)

iTerm2 reads its settings from `~/Library/Preferences`, not `~/.config`, so
these files are kept here for **manual import** rather than symlinking. This dir
carries a `.nostow` marker so `setup.sh` skips it.

- `excid3.itermcolors` — color preset. Import via
  *iTerm2 → Settings → Profiles → Colors → Color Presets → Import…*
- `Profile.json` — exported profile, for reference / re-import.

The actively maintained macOS terminal config is **wezterm** (`../wezterm`,
which *is* stowed to `~/.config/wezterm`).
