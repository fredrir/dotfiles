from dataclasses import dataclass


@dataclass(frozen=True)
class HardwareProfile:
    family: str = ""
    architecture: str = ""
    memory: str = ""
    cores: str = ""
    tdp: int | None = None
    max_temperature: int | None = None


CPU_PROFILES = {
    "AMD Ryzen 7 9800X3D": HardwareProfile(
        family="Granite Ridge X3D",
        tdp=120,
        max_temperature=95,
    ),
}

GPU_PROFILES = {
    "NVIDIA GeForce RTX 5070 Ti": HardwareProfile(
        architecture="Blackwell",
        memory="16 GB GDDR7",
        cores="8,960 CUDA cores",
        max_temperature=88,
    ),
}

DISK_PROFILES = {
    "KINGSTON SNVS2000G": HardwareProfile(
        family="Kingston NV1",
        max_temperature=70,
    ),
    "WDC WD20EZRZ": HardwareProfile(
        family="WD Blue",
        max_temperature=60,
    ),
}


def profile_for(name, profiles):
    lowered = (name or "").lower()
    if not lowered:
        return HardwareProfile()
    for model, profile in profiles.items():
        model_lower = model.lower()
        if model_lower in lowered or lowered in model_lower:
            return profile
    return HardwareProfile()
