# dotfiles

```
                  -`                    fredrir@archpc
                 .o+`                   --------------
                `ooo/                   OS: Arch Linux x86_64
               `+oooo:                  Shell: zsh
              `+oooooo:                 WM: KDE Plasma
              -+oooooo+:                Terminal: konsole
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


## Layout

```
shared/          # (zsh core, nvim, kitty, yazi, git)
linux/
  common/        # Linux: fontconfig, gtk, flameshot, zsh
  kde/           # KDE Plasma only: plasma, kate (KDE themes its own Qt)
  hyprland/      # Hyprland only: hypr, waybar, wofi, wlogout, dunst, elephant, qt5ct, qt6ct, zsh (hypr helpers)
  packages/      # pacman/AUR package lists (reference, not stowed)
macos/           # macOS: zsh (macos fragments), wezterm, iterm (ref), Brewfile
hosts/           # profiles following machine/distro/desktop:
  desktop/arch-linux/kde/              # archpc, KDE only
  desktop/arch-linux/kde-hyprland/     # archpc, KDE + Hyprland
  laptop/arch-linux/hyprland/          # laptop, Hyprland only
  macbook/macos/                       # mac
    manifest          #   groups this profile stows
    hypr-local.conf   #   monitors / GPU env / scale (-> conf.d/local.conf)
    notes.md
```

Each entry a manifest names is a *group* of Stow packages; every package uses the
`<pkg>/.config/<app>/…` layout that Stow symlinks into `$HOME`. A package dir with
a `.nostow` marker is skipped (e.g. `elephant`, which is generated, and `iterm`,
which is import-only).

## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh desktop/arch-linux/kde            # archpc
./setup.sh laptop/arch-linux/hyprland        # laptop
./setup.sh macbook/macos                     # mac
```

`setup.sh` stows every group in the profile's manifest, generates the elephant
config, and links `hosts/<profile>/hypr-local.conf` to
`~/.config/hypr/conf.d/local.conf` (Hyprland profiles).

## Adding a machine

1. Pick the matching profile under `hosts/<machine>/<distro>/<desktop>` — or add
   a new one: `mkdir -p hosts/<machine>/<distro>/<desktop>` with a `manifest`
   listing its groups.
2. (Hyprland) put its monitors / GPU env / scale in
   `hosts/<profile>/hypr-local.conf`. See
   `hosts/desktop/arch-linux/kde-hyprland` (archpc) and
   `hosts/laptop/arch-linux/hyprland` (laptop).
3. `./setup.sh <machine>/<distro>/<desktop>`

## Per-machine settings

- **Monitors / GPU / scale (Hyprland):** `hosts/<profile>/hypr-local.conf`
  (sourced last, overrides shared `conf.d/env.conf`).
- **Shell:** shared zsh fragments in `shared/zsh`; Linux-only in
  `linux/common/zsh`; Hyprland-only in `linux/hyprland/zsh`; macOS-only in
  `macos/zsh`. All co-stow into `~/.config/zsh/conf.d/` and load by numeric order.
- **Wallpaper:** place at `~/.config/hypr/wallpaper.png` (gitignored).
