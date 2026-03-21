`````
                  -`                    fredrir@archpc
                 .o+`                   --------------
                `ooo/                   OS: Arch Linux x86_64
               `+oooo:                  Kernel: Linux 6.19.6-arch1-1
              `+oooooo:                 Packages: 791 (pacman)
              -+oooooo+:                Shell: zsh 5.9
            `/:-:++oooo+:               WM: Hyprland (Wayland)
           `/++++/+++++++:              Terminal: kitty
          `/++++++++++++++:             CPU: AMD Ryzen 7 9800X3D (16) @ 5.27 GHz
         `/+++ooooooooooooo/`           GPU: NVIDIA GeForce GTX 1070
        ./ooosssso++osssssso+`          Memory: 30.51 GiB
       .oossssso-````/ossssss+`
      -osssssso.      :ssssssso.
     :osssssss/        osssso+++.
    /ossssssss/        +ssssooo/-
  `/ossssso+/:-        -:/+osssso+-
 `+sso+:-`                 `.-/+oso:
`++:.                           `-/+/
.`                                 `/
`````

### What's included

| Config     | Description             |
| ---------- | ----------------------- |
| `hypr`     | Hyprland window manager |
| `nvim`     | Neovim editor           |
| `kitty`    | Kitty terminal          |
| `waybar`   | Status bar              |
| `dunst`    | Notification daemon     |
| `wlogout`  | Logout menu             |
| `zsh`      | Shell config            |
| `yazi`     | File manager            |
| `elephant` | File search launcher    |

### Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh
```

### Post-install

- Place your wallpaper at `~/.config/hypr/wallpaper.png`
- Edit `~/.config/hypr/conf.d/local.conf` for machine-specific settings (monitors, etc.)
