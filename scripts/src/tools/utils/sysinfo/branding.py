import re
from dataclasses import dataclass


@dataclass(frozen=True)
class BrandProfile:
    key: str
    name: str
    accent: str
    mark: str
    kinds: tuple[str, ...]
    aliases: tuple[str, ...]


BRANDS = (
    BrandProfile(
        "asus_rog",
        "ASUS ROG",
        "#ff1744",
        "◢R",
        ("motherboard",),
        ("rog strix", "republic of gamers"),
    ),
    BrandProfile("asus_tuf", "ASUS TUF", "#d5a85a", "◥T◤", ("motherboard",), ("tuf gaming",)),
    BrandProfile("nvidia", "NVIDIA", "#76b900", "󰾲", ("gpu",), ("nvidia", "geforce", "quadro")),
    BrandProfile(
        "amd",
        "AMD",
        "#ed1c24",
        "◢◤",
        ("cpu", "gpu"),
        ("authenticamd", "advanced micro devices", "amd", "radeon"),
    ),
    BrandProfile(
        "intel",
        "INTEL",
        "#00c7fd",
        "◉i",
        ("cpu", "gpu", "storage"),
        ("genuineintel", "intel", "arc graphics"),
    ),
    BrandProfile(
        "apple",
        "APPLE",
        "#a7adba",
        "",
        ("cpu", "gpu", "motherboard", "power", "os"),
        ("apple", "macos", "darwin"),
    ),
    BrandProfile(
        "qualcomm", "QUALCOMM", "#3253dc", "Q", ("cpu", "gpu"), ("qualcomm", "snapdragon")
    ),
    BrandProfile("arm", "ARM", "#0091bd", "ARM", ("cpu", "gpu"), ("arm limited", "arm", "mali")),
    BrandProfile("corsair", "CORSAIR", "#f9c900", "≋≋", ("memory", "power", "case"), ("corsair",)),
    BrandProfile("gskill", "G.SKILL", "#e6232b", "G◆", ("memory",), ("g.skill", "g skill")),
    BrandProfile(
        "kingston",
        "KINGSTON",
        "#d71920",
        "K▰",
        ("memory", "storage"),
        ("kingston", "kingston fury", "snvs"),
    ),
    BrandProfile("crucial", "CRUCIAL", "#00a6a6", "C▰", ("memory", "storage"), ("crucial",)),
    BrandProfile("micron", "MICRON", "#0060a9", "M▰", ("memory", "storage"), ("micron",)),
    BrandProfile("samsung", "SAMSUNG", "#4a70d8", "S▰", ("memory", "storage"), ("samsung",)),
    BrandProfile(
        "sk_hynix", "SK HYNIX", "#f25a22", "H▰", ("memory", "storage"), ("sk hynix", "hynix")
    ),
    BrandProfile(
        "teamgroup",
        "TEAMGROUP",
        "#d71920",
        "T▰",
        ("memory", "storage"),
        ("teamgroup", "team group", "t force"),
    ),
    BrandProfile(
        "patriot", "PATRIOT", "#e1262f", "P▰", ("memory", "storage"), ("patriot", "viper")
    ),
    BrandProfile(
        "western_digital",
        "WESTERN DIGITAL",
        "#3daee9",
        "WD",
        ("storage",),
        ("western digital", "wdc", "wd blue", "wd black", "wd red"),
    ),
    BrandProfile(
        "seagate",
        "SEAGATE",
        "#66c011",
        "S◉",
        ("storage",),
        ("seagate", "st2000", "st4000", "st8000"),
    ),
    BrandProfile("sabrent", "SABRENT", "#f28c28", "S▲", ("storage",), ("sabrent", "rocket")),
    BrandProfile("sandisk", "SANDISK", "#ed1c24", "SD", ("storage",), ("sandisk",)),
    BrandProfile("toshiba", "TOSHIBA", "#e60012", "T◉", ("storage",), ("toshiba", "kioxia")),
    BrandProfile("asus", "ASUS", "#3daee9", "A▲", ("motherboard",), ("asustek", "asus")),
    BrandProfile("msi", "MSI", "#e50019", "M◆", ("motherboard",), ("micro-star", "msi")),
    BrandProfile("aorus", "AORUS", "#ff6400", "A◢", ("motherboard",), ("aorus",)),
    BrandProfile("gigabyte", "GIGABYTE", "#f37021", "G▲", ("motherboard",), ("gigabyte",)),
    BrandProfile("asrock", "ASROCK", "#4aa3df", "AS", ("motherboard",), ("asrock",)),
    BrandProfile("evga", "EVGA", "#5abeff", "E◆", ("motherboard", "gpu"), ("evga",)),
    BrandProfile("supermicro", "SUPERMICRO", "#22a6d5", "SM", ("motherboard",), ("supermicro",)),
    BrandProfile("biostar", "BIOSTAR", "#e63329", "B◆", ("motherboard",), ("biostar",)),
    BrandProfile("noctua", "NOCTUA", "#c9a27d", "N✣", ("cooling",), ("noctua",)),
    BrandProfile("arctic", "ARCTIC", "#8fd3ff", "A❄", ("cooling", "case"), ("arctic",)),
    BrandProfile("seasonic", "SEASONIC", "#f0b323", "Sϟ", ("power",), ("seasonic",)),
    BrandProfile(
        "be_quiet",
        "BE QUIET!",
        "#f58220",
        "BQ",
        ("cooling", "case", "power"),
        ("be quiet",),
    ),
    BrandProfile(
        "cooler_master",
        "COOLER MASTER",
        "#7b2cff",
        "CM",
        ("cooling", "case", "power"),
        ("cooler master",),
    ),
    BrandProfile(
        "thermaltake",
        "THERMALTAKE",
        "#ef3123",
        "TT",
        ("cooling", "case", "power"),
        ("thermaltake",),
    ),
    BrandProfile("fractal", "FRACTAL", "#bac2de", "F▣", ("case",), ("fractal",)),
    BrandProfile("nzxt", "NZXT", "#7c3aed", "N▣", ("cooling", "case", "power"), ("nzxt",)),
    BrandProfile("lian_li", "LIAN LI", "#cdd6f4", "LL", ("cooling", "case", "power"), ("lian li",)),
    BrandProfile(
        "dell", "DELL", "#0672ce", "D", ("motherboard", "case", "power"), ("dell", "alienware")
    ),
    BrandProfile(
        "lenovo",
        "LENOVO",
        "#e2231a",
        "L",
        ("motherboard", "case", "power"),
        ("lenovo", "thinkpad", "legion"),
    ),
    BrandProfile(
        "hp",
        "HP",
        "#0096d6",
        "hp",
        ("motherboard", "case", "power"),
        ("hewlett packard", "hp"),
    ),
    BrandProfile(
        "framework",
        "FRAMEWORK",
        "#cdd6f4",
        "FW",
        ("motherboard", "case", "power"),
        ("framework",),
    ),
    BrandProfile(
        "acer",
        "ACER",
        "#83b81a",
        "A",
        ("motherboard", "case", "power"),
        ("acer", "predator"),
    ),
    BrandProfile("razer", "RAZER", "#44d62c", "R", ("motherboard", "case", "power"), ("razer",)),
    BrandProfile("arch", "ARCH LINUX", "#3daee9", "", ("os",), ("arch linux", "arch")),
    BrandProfile("ubuntu", "UBUNTU", "#e95420", "", ("os",), ("ubuntu",)),
    BrandProfile("debian", "DEBIAN", "#d70a53", "", ("os",), ("debian",)),
    BrandProfile("fedora", "FEDORA", "#51a2da", "", ("os",), ("fedora",)),
    BrandProfile("nixos", "NIXOS", "#7ebae4", "", ("os",), ("nixos", "nix os")),
    BrandProfile("windows", "WINDOWS", "#00a4ef", "", ("os",), ("windows",)),
    BrandProfile(
        "kde", "KDE PLASMA", "#3daee9", "", ("desktop", "wm"), ("kde plasma", "plasma", "kwin")
    ),
    BrandProfile("gnome", "GNOME", "#4a86cf", "", ("desktop",), ("gnome",)),
    BrandProfile("hyprland", "HYPRLAND", "#58e1ff", "H◇", ("desktop", "wm"), ("hyprland",)),
    BrandProfile("sway", "SWAY", "#6f838c", "S◇", ("desktop", "wm"), ("sway",)),
    BrandProfile("xfce", "XFCE", "#00a6d6", "X◇", ("desktop",), ("xfce",)),
    BrandProfile("cinnamon", "CINNAMON", "#8fcf3c", "C◇", ("desktop",), ("cinnamon",)),
    BrandProfile("wayland", "WAYLAND", "#f38ba8", "W", ("session",), ("wayland",)),
    BrandProfile("x11", "X11", "#bac2de", "X", ("session",), ("x11", "xorg")),
    BrandProfile("konsole", "KONSOLE", "#3daee9", ">_", ("terminal",), ("konsole",)),
    BrandProfile("kitty", "KITTY", "#f9e2af", "K>", ("terminal",), ("kitty",)),
    BrandProfile("ghostty", "GHOSTTY", "#b4befe", "G>", ("terminal",), ("ghostty",)),
    BrandProfile("alacritty", "ALACRITTY", "#f46f25", "A>", ("terminal",), ("alacritty",)),
    BrandProfile("wezterm", "WEZTERM", "#7cff6b", "W>", ("terminal",), ("wezterm",)),
    BrandProfile("foot", "FOOT", "#7cff6b", "F>", ("terminal",), ("foot", "footclient")),
    BrandProfile("iterm", "ITERM2", "#7cff6b", "I>", ("terminal",), ("iterm2", "iterm")),
    BrandProfile(
        "gnome_terminal", "GNOME TERMINAL", "#4a86cf", "G>", ("terminal",), ("gnome terminal",)
    ),
    BrandProfile("zsh", "ZSH", "#7cff6b", "Z>", ("shell",), ("zsh",)),
    BrandProfile("bash", "BASH", "#7cff6b", "B>", ("shell",), ("bash",)),
    BrandProfile("fish", "FISH", "#7cff6b", "F>", ("shell",), ("fish",)),
)

GENERIC = {
    "cpu": BrandProfile("cpu", "PROCESSOR", "#fab387", "◆", ("cpu",), ()),
    "gpu": BrandProfile("gpu", "GRAPHICS", "#7cff6b", "◈", ("gpu",), ()),
    "memory": BrandProfile("memory", "MEMORY", "#f9e2af", "≋", ("memory",), ()),
    "motherboard": BrandProfile("motherboard", "MAINBOARD", "#3daee9", "◇", ("motherboard",), ()),
    "storage": BrandProfile("storage", "STORAGE", "#1abc9c", "▰", ("storage",), ()),
    "cooling": BrandProfile("cooling", "COOLING", "#8fd3ff", "❄", ("cooling",), ()),
    "case": BrandProfile("case", "CHASSIS", "#bac2de", "▣", ("case",), ()),
    "power": BrandProfile("power", "POWER", "#f9e2af", "ϟ", ("power",), ()),
    "os": BrandProfile("os", "SYSTEM", "#3daee9", "◆", ("os",), ()),
    "desktop": BrandProfile("desktop", "DESKTOP", "#cba6f7", "◇", ("desktop",), ()),
    "wm": BrandProfile("wm", "COMPOSITOR", "#cba6f7", "◇", ("wm",), ()),
    "session": BrandProfile("session", "SESSION", "#f38ba8", "◇", ("session",), ()),
    "terminal": BrandProfile("terminal", "TERMINAL", "#7cff6b", ">_", ("terminal",), ()),
    "shell": BrandProfile("shell", "SHELL", "#7cff6b", ">", ("shell",), ()),
}

ART = {
    ("amd", "cpu"): ("  ◢██", " ◢  █", "◢██◤ "),
    ("amd", "gpu"): ("◢█  █", "  ◢█ ", "◢██◤ "),
    ("nvidia", "gpu"): ("▟━━━▙", "┃ ◉◉┃", "▀━━━▀"),
    ("corsair", "memory"): ("╱╱╱╱ ", "█████", "╵╵╵╵╵"),
    ("asus_tuf", "motherboard"): ("◥███◤", " ╲◆╱ ", "  ▼  "),
    ("asus_rog", "motherboard"): ("◢██◤ ", "  ◢R ", "◢██◤ "),
    ("asus", "motherboard"): ("  ▲  ", " A██ ", "◢██◤ "),
    ("kingston", "storage"): ("▟━━━▙", "┃ ▰ ┃", "╵╵╵╵╵"),
    ("western_digital", "storage"): ("  ◉  ", "█████", "╵╵╵╵╵"),
    ("samsung", "storage"): ("▟ S ▙", "█████", "╵╵╵╵╵"),
    ("noctua", "cooling"): ("╲ │ ╱", "─ ◉ ─", "╱ │ ╲"),
    ("arctic", "case"): ("▟ A ▙", "█   █", "▀███▀"),
    ("corsair", "power"): ("  ϟ  ", "▰▰▰▰▰", "╵╵╵╵╵"),
    ("arch", "os"): ("   ▲ ", "  ╱ ╲", " ╱╱ ╲╲"),
}

HEADER_ART = {
    "arch": (
        "                 /\\",
        "                /  \\",
        "               /    \\",
        "              /      \\",
        "             /\\       \\",
        "            /  \\       \\",
        "           /            \\",
        "          /      /\\      \\",
        "         /      /  \\      \\",
        "        /      /    \\      \\",
        "       /__--'''      '''--___\\",
    ),
}

GENERIC_HEADER_ART = (
    "       ◢████◣",
    "     ◢████████◣",
    "    ███◤    ◥███",
    "   ███        ███",
    "   ███   ◆    ███",
    "   ███        ███",
    "    ███◣    ◢███",
    "     ◥████████◤",
    "       ◥████◤",
)

GENERIC_ART = {
    "cpu": ("╋━━━╋", "┃ ◆ ┃", "╋━━━╋"),
    "gpu": ("▟━━━▙", "┃ ◉ ┃", "▀━━━▀"),
    "memory": ("▟━━━▙", "█████", "╵╵╵╵╵"),
    "motherboard": ("▟━━━▙", "┃◇◆┃", "▀━━━▀"),
    "storage": ("▟━━━▙", "┃ ▰ ┃", "▀━━━▀"),
    "cooling": ("╲ │ ╱", "─ ◉ ─", "╱ │ ╲"),
    "case": ("▟━━━▙", "┃   ┃", "▀━━━▀"),
    "power": ("  ϟ  ", "▰▰▰▰▰", "╵╵╵╵╵"),
    "os": ("  ◆  ", " ◆◆◆ ", "◆   ◆"),
}


def normalized(value):
    return " ".join(re.sub(r"[^a-z0-9]+", " ", value.lower()).split())


def alias_matches(alias, haystack):
    target = normalized(alias)
    return bool(target) and f" {target} " in f" {haystack} "


def resolve_brand(kind, *values):
    haystack = normalized(" ".join(str(value) for value in values if value))
    for profile in BRANDS:
        if kind not in profile.kinds:
            continue
        if any(alias_matches(alias, haystack) for alias in profile.aliases):
            return profile
    return GENERIC[kind]


def illustration(profile, kind):
    return ART.get((profile.key, kind), GENERIC_ART.get(kind, (profile.mark,)))


def header_illustration(profile):
    return HEADER_ART.get(profile.key, GENERIC_HEADER_ART)


def strip_brand(model, profile):
    value = model.strip()
    prefixes = sorted((profile.name, *profile.aliases), key=len, reverse=True)
    for prefix in prefixes:
        pattern = rf"^{re.escape(prefix)}(?:\s+|$)"
        value = re.sub(pattern, "", value, flags=re.IGNORECASE).strip()
    return value or model.strip()


def validate_registry():
    keys = [profile.key for profile in BRANDS]
    if len(keys) != len(set(keys)):
        raise ValueError("brand keys must be unique")
    for profile in BRANDS:
        if not profile.kinds or not profile.aliases:
            raise ValueError(f"incomplete brand profile: {profile.key}")


validate_registry()
