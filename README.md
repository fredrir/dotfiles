# dotfiles

```
                  -`                    fredrir@archpc
                 .o+`                   --------------
                `ooo/                   OS: Arch Linux x86_64
               `+oooo:                  Shell: zsh
              `+oooooo:                 WM: Hyprland (Wayland) / KDE Plasma
              -+oooooo+:                Terminal: kitty / wezterm
            `/:-:++oooo+:               Editor: neovim
           `/++++/+++++++:
          `/++++++++++++++:
         `/+++ooooooooooooo/`
        ./ooosssso++osssssso+`
       .oossssso-````/ossssss+`
      -osssssso.      :ssssssso.
     :osssssss/        osssso+++.
    /ossssssss/        +ssssooo/-
  `/ossssso+/:-        -:/+osssso+-
 `+sso+:-`                 `.-/+oso:
`++:.                           `-/+/
.`                                 `/
```

One branch, many machines. Configs are grouped by where they apply, and each
machine declares which groups it wants. Managed with [GNU Stow](https://www.gnu.org/software/stow/).

## Layout

```
shared/          # stowed on every machine (zsh core, nvim, kitty, yazi, git)
linux/
  common/        # all Linux: fontconfig, gtk, qt5ct, qt6ct, flameshot, zsh (linux fragments)
  kde/           # KDE Plasma only: plasma, kate
  hyprland/      # Hyprland only: hypr, waybar, wofi, wlogout, dunst, elephant
  packages/      # pacman/AUR package lists (reference, not stowed)
macos/           # macOS only: zsh (macos fragments), wezterm, iterm (ref), Brewfile
hosts/           # per-machine: manifest + tracked overrides + notes
  <host>/manifest          # which groups to stow on this host
  <host>/hypr-local.conf   # host monitors / GPU env / scale (symlinked to conf.d/local.conf)
```

Each entry a manifest names is a *group* of Stow packages; every package uses the
`<pkg>/.config/<app>/…` layout that Stow symlinks into `$HOME`. A package dir with
a `.nostow` marker is skipped (e.g. `elephant`, which is generated, and `iterm`,
which is import-only).

## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh            # uses this machine's hostname → hosts/<hostname>/manifest
# or target a host explicitly:
./setup.sh archpc
```

`setup.sh` stows every group in the host's manifest, generates the elephant
config, and links `hosts/<host>/hypr-local.conf` to
`~/.config/hypr/conf.d/local.conf` (Hyprland hosts).

## Adding a machine

1. `mkdir hosts/<hostname>` and add a `manifest` listing groups, e.g.
   `shared`, `linux/common`, `linux/hyprland`.
2. (Hyprland) add `hosts/<hostname>/hypr-local.conf` with its monitors / GPU env
   / scale. See `hosts/archpc` and `hosts/laptop` for examples.
3. `./setup.sh <hostname>`

## Per-machine settings

- **Monitors / GPU / scale (Hyprland):** `hosts/<host>/hypr-local.conf`
  (sourced last, overrides shared `conf.d/env.conf`).
- **Shell:** shared zsh fragments in `shared/zsh`; Linux-only in
  `linux/common/zsh`; macOS-only in `macos/zsh`. All co-stow into
  `~/.config/zsh/conf.d/` and load by numeric order.
- **Wallpaper:** place at `~/.config/hypr/wallpaper.png` (gitignored).
