from tools.utils import sysinfo

FASTFETCH_DATA = [
    {"type": "OS", "result": {"prettyName": "Arch Linux"}},
    {"type": "Kernel", "result": {"release": "6.15.1-arch1-1"}},
    {"type": "Shell", "result": {"prettyName": "zsh", "version": "5.9"}},
    {"type": "CPU", "result": {"cpu": "AMD Ryzen 7 9800X3D"}},
    {
        "type": "GPU",
        "result": [
            {"name": "AMD Radeon Graphics", "vendor": "AMD", "type": "Integrated"},
            {"name": "GeForce RTX 5070 Ti", "vendor": "NVIDIA", "type": "Discrete"},
        ],
    },
    {"type": "Memory", "result": {"total": 33538248704}},
    {"type": "Disk", "result": [{"bytes": {"total": 2000000000000}}]},
    {"type": "DE", "result": {"prettyName": "KDE Plasma", "version": "6.3.5"}},
    {"type": "WM", "result": {"prettyName": "KWin", "protocolName": "Wayland"}},
    {"type": "Terminal", "result": {"prettyName": "konsole", "version": "25.04.1"}},
    {
        "type": "Board",
        "result": {"name": "ROG STRIX B650E-F", "vendor": "ASUSTeK COMPUTER INC."},
    },
    {
        "type": "PhysicalDisk",
        "result": [
            {"name": "Samsung SSD 990 PRO 2TB", "size": 2000398934016, "interconnect": "NVMe"}
        ],
    },
]


def test_capacity_matches_jq_formatting():
    assert sysinfo.capacity(2000398934016) == "2 TB"
    assert sysinfo.capacity(1850000000000) == "1.9 TB"
    assert sysinfo.capacity(512110190592) == "512.1 GB"
    assert sysinfo.capacity(157286400) == "157.3 MB"
    assert sysinfo.capacity(0) == "unknown"
    assert sysinfo.capacity(None) == "unknown"


def test_memory_capacity_rounds_up_to_modules():
    assert sysinfo.memory_capacity(33538248704) == "32 GB"
    assert sysinfo.memory_capacity(68719476736) == "64 GB"
    assert sysinfo.memory_capacity(0) == "unknown"


def test_named_gpu_prefixes_the_vendor():
    assert sysinfo.named_gpu({"name": "GeForce RTX 5070 Ti", "vendor": "NVIDIA"}) == (
        "NVIDIA GeForce RTX 5070 Ti"
    )
    assert sysinfo.named_gpu({"name": "NVIDIA T400", "vendor": "NVIDIA"}) == "NVIDIA T400"
    assert sysinfo.named_gpu({}) == "unknown"


def test_full_output_lines(monkeypatch, capsys, tmp_path):
    config = tmp_path / "hardware.dotfile"
    config.write_text(
        "desktop {\n    CPU_COOLER=Noctua NH-D15\n    MEMORY=Corsair 32 GB DDR5-6000\n}\n"
    )
    monkeypatch.setenv("SYSINFO_CONFIG", str(config))
    monkeypatch.setenv("SYSINFO_HARDWARE", "desktop")
    monkeypatch.setattr(sysinfo, "nvidia_vram", lambda: "16 GB")
    monkeypatch.setattr(sysinfo, "shell_info", lambda: ("zsh", "5.9"))
    monkeypatch.setattr(sysinfo, "terminal_info", lambda: ("kitty", "0.40.1"))
    monkeypatch.setattr(sysinfo, "collect_fastfetch", lambda: FASTFETCH_DATA)

    sysinfo.sysinfo()
    lines = capsys.readouterr().out.splitlines()
    assert lines[0] == (
        "Environment: OS=Arch Linux, Kernel=6.15.1-arch1-1, Shell=zsh 5.9, "
        "CPU=AMD Ryzen 7 9800X3D, GPU=NVIDIA GeForce RTX 5070 Ti, Memory=32 GB, "
        "Disk=2 TB, DE=KDE Plasma 6.3.5, WM=KWin (Wayland), Terminal=kitty 0.40.1"
    )
    assert lines[1] == (
        "Hardware: GPU=NVIDIA GeForce RTX 5070 Ti 16 GB, CPU=AMD Ryzen 7 9800X3D, "
        "CPU cooler=Noctua NH-D15, Motherboard=ASUS ROG STRIX B650E-F, "
        "Memory=Corsair 32 GB DDR5-6000, Storage=Samsung SSD 990 PRO 2TB 2 TB NVMe, "
        "Case=not set, Power supply=not set"
    )
