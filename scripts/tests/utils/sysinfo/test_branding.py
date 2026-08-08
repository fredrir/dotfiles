import pytest

from tools.utils.sysinfo.branding import header_illustration, illustration, resolve_brand


@pytest.mark.parametrize(
    ("kind", "identity", "expected"),
    [
        ("cpu", "AuthenticAMD Ryzen 9 10950X3D", "amd"),
        ("gpu", "NVIDIA GeForce RTX 6090", "nvidia"),
        ("gpu", "Intel Arc B990", "intel"),
        ("memory", "Corsair Dominator Titanium DDR5", "corsair"),
        ("memory", "G.Skill Trident Z5", "gskill"),
        ("memory", "SK hynix DDR5", "sk_hynix"),
        ("motherboard", "ASUSTeK TUF GAMING X990", "asus_tuf"),
        ("motherboard", "ASUS ROG STRIX X990", "asus_rog"),
        ("motherboard", "Gigabyte AORUS MASTER", "aorus"),
        ("storage", "KINGSTON SNVS2000G", "kingston"),
        ("storage", "ATA WDC WD20EZRZ", "western_digital"),
        ("storage", "Samsung SSD 990 PRO", "samsung"),
        ("os", "Arch Linux", "arch"),
        ("desktop", "KDE Plasma", "kde"),
        ("session", "Wayland", "wayland"),
        ("terminal", "Ghostty", "ghostty"),
        ("shell", "zsh 5.9", "zsh"),
    ],
)
def test_brand_registry_covers_hardware_and_software(kind, identity, expected):
    assert resolve_brand(kind, identity).key == expected


def test_unknown_hardware_uses_device_class_fallback():
    profile = resolve_brand("gpu", "Future Silicon Company", "Photon 9000")

    assert profile.key == "gpu"
    assert profile.name == "GRAPHICS"
    assert illustration(profile, "gpu")


def test_current_vendor_illustrations_are_distinct():
    combinations = (
        ("cpu", "AMD Ryzen 7 9800X3D"),
        ("gpu", "NVIDIA GeForce RTX 5070 Ti"),
        ("memory", "Corsair DDR5"),
        ("motherboard", "ASUS TUF GAMING B850"),
        ("storage", "KINGSTON SNVS2000G"),
        ("storage", "WDC WD20EZRZ"),
    )
    arts = {illustration(resolve_brand(kind, identity), kind) for kind, identity in combinations}

    assert len(arts) == len(combinations)


def test_arch_header_illustration_is_complete():
    art = header_illustration(resolve_brand("os", "Arch Linux"))

    assert len(art) == 11
    assert art[0].strip() == "/\\"
    assert art[-1].strip().startswith("/__--'''")
    assert art[-1].strip().endswith("___\\")
