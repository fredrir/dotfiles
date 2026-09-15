# archie install

Clean install on the Crucial T710 4 TB. Windows and the Kingston KC3000 leave the machine.

## Before install, from the live USB

| Step | Command |
| --- | --- |
| find the T710 | `nvme list` |
| LBA formats | `nvme id-ns -H /dev/nvme0n1 \| grep "LBA Format"` |
| switch to 4096-byte sectors (destroys data) | `nvme format /dev/nvme0n1 --lbaf=<index of 4096> --ses=0` |
| firmware | `fwupdmgr get-devices` |

## Partitions

| Partition | Size | Type | Mount |
| --- | --- | --- | --- |
| 1 | 1 GiB | `ef00` EFI System | `/efi` |
| 2 | rest | `8304` Linux root (x86-64) | `/` |

```
sgdisk --zap-all /dev/nvme0n1
sgdisk -n 1:0:+1G -t 1:ef00 -n 2:0:0 -t 2:8304 /dev/nvme0n1
mkfs.fat -F 32 /dev/nvme0n1p1
mkfs.ext4 /dev/nvme0n1p2
mount /dev/nvme0n1p2 /mnt
mount --mkdir /dev/nvme0n1p1 /mnt/efi
```

| Not used | Why |
| --- | --- |
| swap partition | zram-generator |
| separate `/home` | single root by choice |
| encryption | by choice |
| `root=` on the kernel command line | GPT type `8304` is auto-discovered by the systemd initramfs hook |

## Base system

| Step | Command |
| --- | --- |
| packages | `pacstrap -K /mnt base linux linux-firmware amd-ucode nvidia-open nvidia-utils $(grep -v ^# environment/arch-linux/kde/pkglist.txt)` |
| fstab | `genfstab -U /mnt >> /mnt/etc/fstab` |
| boot loader | `arch-chroot /mnt bootctl install` |
| UKI | `arch-chroot /mnt mkinitcpio -P` after `linux/arch/kernel` is installed |
| AUR | `yay -S --needed $(grep -v ^# environment/arch-linux/kde/aurlist.txt)` as the user |

## Post-install, as the user

| Step | Command |
| --- | --- |
| dotfiles | `git clone <repo> ~/dotfiles && cd ~/dotfiles && ./setup.sh` |
| user configs | `dotfile sync` |
| root-owned configs | `dotfile system install` |
| UKI with the tracked command line | `sudo mkinitcpio -P` |
| services | `sudo systemctl enable --now fan2go lactd nvidia-persistenced fstrim.timer macie-usb-dhcp` |
| user services | `systemctl --user enable --now wezterm-mux wezterm-mux-route-lan wezterm-mux-dial-lan` |
| mux without a login session | `sudo loginctl enable-linger fredrir` |
| energy counters without reboot | `sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=powercap --action=add` |
| sysctl without reboot | `sudo sysctl --system` |
| verify | `hwtune status`, `hwtune bios check`, `dotfile doctor` |
| baseline | `hwtune bench run --baseline` (the disk change moves the hardware epoch) |

| Kernel command line | Why |
| --- | --- |
| `zswap.enabled=0` | swap is zram; zswap in front of it compresses twice |
| `cpuidle.governor=teo` | shorter idle exit latency than `menu` on Zen 5 |

| Tuning package | Path |
| --- | --- |
| `linux/arch/tuning-sysctl` | `/etc/sysctl.d/40-tuning.conf` |
| `linux/arch/rapl` | `/etc/udev/rules.d/70-rapl-energy.rules` |
| `linux/arch/cargo` | `~/.cargo/config.toml`; needs `mold` and `sccache` installed first |

## Replacing the UKI on a running system

| Step | Command |
| --- | --- |
| keep the current UKI bootable | `sudo cp /efi/EFI/Linux/arch-linux.efi /efi/EFI/Linux/arch-linux-previous.efi` |
| install the tracked files | `dotfile system install` |
| rebuild | `sudo mkinitcpio -P` |
| after a good boot | `sudo rm /efi/EFI/Linux/arch-linux-previous.efi` |

## BIOS

| Step | Where |
| --- | --- |
| load the saved profile | Tool → ASUS User Profile → Load from USB |
| export the settings | Tool → ASUS User Profile → save the text export to USB |
| verify | `hwtune bios import /mnt/usb/Archie_BIOS_setting.txt && hwtune bios check` |
| boot order | the UKI only; Windows entries are gone with the Kingston |

## Memory ladder

One change per reboot; every step is `hwtune stress mem --tool y-cruncher --minutes 30`, then export, `hwtune bios import`, `hwtune bench run --only mem,compile --note "<step>"`, and `hwtune bench report --before <run> --after <run>`.

| Step | Setting | From | To | Guard |
| --- | --- | --- | --- | --- |
| 1 | Refresh Interval (tREFI) | Auto | 65535 | DIMM temp below 55 °C in `sensors spd5118-*` under stress |
| 2 | Trfc1 / Trfc2 / Trfcsb | Auto | 520 / 520 / 400 | y-cruncher pass; step to 500 / 500 / 380 next |
| 3 | TrrdS / TrrdL / Tfaw | Auto | 8 / 12 / 32 | y-cruncher pass |
| 4 | Twr / Trtp | Auto | 48 / 12 | y-cruncher pass |
| 5 | TwtrS / TwtrL | Auto | 4 / 24 | y-cruncher pass |
| 6 | TrdrdScl / TwrwrScl | Auto | 5 / 5 | y-cruncher pass; 4 / 4 next |
| 7 | FCLK Frequency | Auto | 2100 | `mem.latency` improves; WHEA-free journal |
| 8 | Power Down Enable | Enabled | Disabled | idle package power in `hwtune bench run --only idle` |

| Voltage | Ceiling |
| --- | --- |
| CPU SOC Voltage | 1.30 V |
| DRAM VDD / VDDQ | 1.40 V as set by EXPO; 1.45 V only for step 7 |
| Memory Context Restore | Enabled; Disabled if training fails after a step |

## Curve Optimizer ladder

| Step | Command |
| --- | --- |
| current evidence and suggestion | `hwtune curve status` |
| throughput sample before a change | `hwtune curve bench` |
| BIOS | Ai Tweaker → Curve Optimizer → Per Core; set the suggested magnitudes |
| export and import | `hwtune bios import /mnt/usb/Archie_BIOS_setting.txt` |
| stress the changed cores | `hwtune stress cpu --profile per-core --minutes 10 --cores 0-7 --offset -30` |
| clock-stretch check | `hwtune curve bench`, then `hwtune curve status` |

| Rule | Value |
| --- | --- |
| step size | -5 until a core fails, then back off to the shallowest passing value |
| prefcore | the two highest ranked cores usually hold the least negative offset |
| stretching | a core whose single-thread throughput drops more than 3 % at a passing offset is not stable |
| reboot during per-core stress | recorded as a failure for the core under test |

## GPU power cap

| Step | Command |
| --- | --- |
| sweep | `hwtune gpu sweep --caps 250,275,300,325,350` |
| keep a cap | edit `power_cap` in `linux/arch/lact/etc/lact/config.yaml`, then `dotfile system install` |
| verify | `hwtune status` shows the cap; `hwtune bench run --only ai` matches the sweep |
