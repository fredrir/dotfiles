# archpc

Arch Linux desktop.

- GPU: NVIDIA GeForce GTX 1070 → NVIDIA env vars in `hypr-local.conf`.
- Display: 4K @ 1.5 scale (`GDK_SCALE=1.5`). Adjust the `monitor =` line in
  `hypr-local.conf` to your actual connector (run `hyprctl monitors`).
- Desktops: both KDE Plasma and Hyprland installed.

Install: `./setup.sh archpc`
