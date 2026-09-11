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
| services | `sudo systemctl enable --now fan2go lactd nvidia-persistenced fstrim.timer` |
| verify | `hwtune status`, `hwtune bios check`, `dotfile doctor` |
| baseline | `hwtune bench run --baseline` (the disk change moves the hardware epoch) |

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
