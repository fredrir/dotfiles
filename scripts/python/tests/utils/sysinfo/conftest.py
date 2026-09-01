import pytest

from tools.utils.sysinfo.models import Snapshot


@pytest.fixture
def workstation_snapshot():
    return Snapshot(
        hardware={
            "cpu_cooler": "Noctua NH-D15",
            "memory": "Corsair CMK32GX5M2B6000Z30 32 GB (2×16 GB) DDR5-6000 CL30",
            "case": "ARCTIC Xtender ATX Mid Tower",
            "power_supply": "Corsair RM1000e 1000 W",
        },
        modules={
            "OS": {"id": "arch", "name": "Arch Linux", "prettyName": "Arch Linux"},
            "Host": {"vendor": "ASUS", "family": "Desktop", "name": "archie"},
            "Kernel": {"release": "7.1.5-arch1-2", "architecture": "x86_64"},
            "Uptime": {"uptime": 93784000},
            "Packages": {"all": 1315, "pacman": 1315},
            "CPU": {
                "cpu": "AMD Ryzen 7 9800X3D",
                "vendor": "AuthenticAMD",
                "cores": {"physical": 8, "logical": 16},
                "frequency": {"max": 5271},
                "temperature": 51.5,
                "codeName": "Ryzen 7 (Granite Ridge)",
                "march": "x86_64-v4",
                "technology": "TSMC N4",
            },
            "CPUCache": {"l3": [{"size": 100663296, "num": 1}]},
            "CPUUsage": [4.0] * 16,
            "GPU": [
                {
                    "name": "Radeon Graphics",
                    "vendor": "AMD",
                    "type": "Integrated",
                    "coreUsage": 1.0,
                    "temperature": 45.0,
                    "frequency": 2200,
                    "driver": "amdgpu",
                    "memory": {"dedicated": {"used": 32 * 1024**2, "total": 512 * 1024**2}},
                },
                {
                    "index": 0,
                    "name": "NVIDIA GeForce RTX 5070 Ti",
                    "vendor": "NVIDIA",
                    "type": "Discrete",
                    "driver": "nvidia 610.43.03",
                    "pcieSpeed": {"max": {"gen": 5, "lanes": 16}},
                },
            ],
            "OpenCL": {"name": "NVIDIA CUDA", "version": "3.0 CUDA 13.3.44"},
            "Vulkan": {"apiVersion": "1.4.341", "driver": "NVIDIA 610.43.03"},
            "Memory": {"used": 8 * 1024**3, "total": 32 * 1024**3},
            "Swap": [],
            "Disk": [
                {
                    "mountpoint": "/",
                    "filesystem": "ext4",
                    "bytes": {"used": 42 * 1024**3, "total": 128 * 1024**3},
                }
            ],
            "PhysicalDisk": [
                {
                    "name": "KINGSTON SNVS2000G",
                    "size": 2000398934016,
                    "interconnect": "NVMe",
                    "kind": "SSD",
                    "temperature": 31.0,
                    "serial": "PRIVATE-DISK-SERIAL",
                },
                {
                    "name": "ATA WDC WD20EZRZ-00Z",
                    "size": 2000398934016,
                    "interconnect": "ATA",
                    "kind": "HDD",
                    "serial": "PRIVATE-HDD-SERIAL",
                },
            ],
            "DE": {"prettyName": "KDE Plasma", "version": "6.7.3"},
            "WM": {
                "prettyName": "KWin",
                "protocolName": "Wayland",
                "processName": "kwin_wayland",
            },
            "Theme": {"theme1": "Breeze (Dark) [Qt]"},
            "Display": [
                {
                    "name": "LU28R55",
                    "output": {"width": 3840, "height": 2160, "refreshRate": 60.0},
                    "scaled": {"width": 2560, "height": 1440},
                    "hdrStatus": "Supported",
                    "serial": "PRIVATE-DISPLAY-SERIAL",
                }
            ],
            "Board": {
                "vendor": "ASUSTeK COMPUTER INC.",
                "name": "TUF GAMING B850-PLUS WIFI",
                "version": "Rev 1.xx",
                "serial": "PRIVATE-BOARD-SERIAL",
            },
            "BIOS": {"type": "UEFI", "version": "1681"},
            "Bootmgr": {"name": "Arch Linux final", "secureBoot": False},
            "InitSystem": {"name": "systemd", "version": "261.2-1-arch"},
        },
        shell_display="zsh 5.9.2",
        terminal_display="Konsole 26.04.3",
        de_display="KDE Plasma 6.7.3",
        wm_display="KWin (Wayland)",
        nvidia=(
            {
                "index": 0,
                "name": "NVIDIA GeForce RTX 5070 Ti",
                "memory_total_mib": 16303.0,
                "memory_used_mib": 2048.0,
                "utilization": 8.0,
                "temperature": 39.0,
                "power_draw": 32.5,
                "power_limit": 300.0,
                "clock_mhz": 2805.0,
                "driver": "610.43.03",
            },
        ),
    )


@pytest.fixture
def macos_snapshot():
    return Snapshot(
        hardware={
            "cpu_cooler": "not set",
            "case": "not set",
            "power_supply": "not set",
        },
        modules={
            "OS": {"id": "macos", "name": "macOS", "prettyName": "macOS 26.0"},
            "Kernel": {"release": "25.0.0", "architecture": "arm64"},
            "CPU": {
                "cpu": "Apple M5 Pro",
                "vendor": "Apple",
                "cores": {"physical": 15, "logical": 15},
                "frequency": {"max": 4610},
                "temperature": 40.5,
                "march": "ARMv9.3-A",
            },
            "CPUUsage": [8.0] * 15,
            "GPU": [
                {
                    "name": "Apple M5 Pro",
                    "vendor": "Apple",
                    "type": "Integrated",
                    "coreUsage": 21.0,
                    "temperature": 41.0,
                    "frequency": 1620,
                    "driver": "com.apple.AGXG17X 351.2",
                }
            ],
            "Memory": {"used": int(18.4 * 1024**3), "total": 24 * 1024**3},
            "PhysicalMemory": [
                {
                    "manufacturer": "Apple",
                    "type": "LPDDR5X",
                }
            ],
            "Swap": [],
            "Disk": [
                {
                    "mountpoint": "/",
                    "filesystem": "apfs",
                    "bytes": {"used": 180 * 1024**3, "total": 994 * 1024**3},
                },
                {
                    "mountpoint": "/System/Volumes/Preboot",
                    "filesystem": "apfs",
                    "bytes": {"used": 9.9 * 1024**3, "total": 10 * 1024**3},
                },
                {
                    "name": "Apple Disk Image Media",
                    "mountpoint": "/Volumes/Installer",
                    "filesystem": "apfs",
                    "volumeType": ["Virtual", "Read-only"],
                    "bytes": {"used": 18 * 1024**3, "total": 18 * 1024**3},
                },
            ],
            "PhysicalDisk": [
                {
                    "name": "APPLE SSD AP1024Z Media",
                    "size": 1000555581440,
                    "interconnect": "Apple Fabric",
                    "kind": "SSD",
                    "temperature": 35.0,
                },
                {
                    "name": "Apple Disk Image Media",
                    "size": 18100000000,
                    "interconnect": "Virtual Interface - File",
                    "kind": "Virtual",
                },
                {
                    "name": "Apple Disk Image Media",
                    "size": 9100000000,
                    "interconnect": "Virtual Interface - File",
                    "kind": "Virtual",
                },
            ],
            "Board": {"vendor": "Apple", "name": "J714sAP"},
            "WM": {"prettyName": "Quartz Compositor"},
            "Battery": [
                {
                    "modelName": "bq40z651",
                    "manufacturer": "Apple",
                    "capacity": 100.0,
                    "status": ["AC Connected"],
                    "cycleCount": 3,
                }
            ],
            "PowerAdapter": [
                {
                    "name": "0",
                    "manufacturer": "Apple",
                    "watts": 70,
                }
            ],
        },
        shell_display="zsh 5.9.2",
        terminal_display="unknown",
        de_display="unknown",
        wm_display="Quartz Compositor",
        nvidia=(),
    )
