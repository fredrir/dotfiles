import os

from tools.core import blocks
from tools.core.dotfmt import formatted
from tools.dotfile.state import die, log, manifest_groups, sorted_entries

DEFAULT_GROUPS = [
    "shared",
    "linux/common",
    "linux/arch",
    "linux/ubuntu",
    "linux/kde",
    "linux/hyprland",
    "linux/server",
    "macos",
]

GROUP_CHARS = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/-")
PACKAGE_CHARS = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._+@-")


def read_package_entries(ctx):
    try:
        return blocks.read(ctx.packages_config, comments=False, open_suffix=" {")
    except blocks.BlockError as error:
        die(blocks.describe(error, "config/packages.dotfile", "group"))
        return []


def load_package_metadata(ctx):
    ctx.package_descriptions = {}
    if not os.path.isfile(ctx.packages_config):
        return
    for entry in read_package_entries(ctx):
        if entry.opens:
            if not entry.block:
                die(f"config/packages.dotfile:{entry.number}: empty group")
            if set(entry.block) - GROUP_CHARS:
                die(f"config/packages.dotfile:{entry.number}: invalid group: {entry.block}")
            continue
        name, description = entry.split(" = ")
        if not name:
            die(f"config/packages.dotfile:{entry.number}: empty package")
        if set(name) - PACKAGE_CHARS:
            die(f"config/packages.dotfile:{entry.number}: invalid package: {name}")
        key = f"{entry.block}/{name}"
        if key in ctx.package_descriptions:
            die(f"config/packages.dotfile:{entry.number}: duplicate package: {key}")
        ctx.package_descriptions[key] = description


def package_groups(ctx):
    groups = list(DEFAULT_GROUPS)
    if os.path.isdir(ctx.environment_dir):
        manifests = []
        for parent, _dirnames, filenames in os.walk(ctx.environment_dir):
            if "manifest" in filenames:
                manifests.append(os.path.join(parent, "manifest"))
        for manifest in sorted(manifests):
            groups.extend(manifest_groups(manifest))
    seen = set()
    unique = []
    for group in groups:
        if group and group not in seen:
            seen.add(group)
            unique.append(group)
    return unique


def group_packages(directory):
    return [
        name for name in sorted_entries(directory) if os.path.isdir(os.path.join(directory, name))
    ]


def validate_package_names(ctx):
    for group in package_groups(ctx):
        directory = os.path.join(ctx.root, group)
        if not os.path.isdir(directory):
            continue
        for pkg in group_packages(directory):
            if pkg == "overrides":
                continue
            if set(pkg) - PACKAGE_CHARS:
                die(f"package directory has an unsupported name: {group}/{pkg}")


def render_packages(ctx):
    config_parts = []
    doc_parts = []
    wrote_group = False
    for group in package_groups(ctx):
        directory = os.path.join(ctx.root, group)
        if not os.path.isdir(directory):
            continue
        packages = [pkg for pkg in group_packages(directory) if pkg != "overrides"]
        if not packages:
            continue
        if wrote_group:
            config_parts.append("\n")
        config_parts.append(f"{group} {{\n")
        doc_parts.append(f"\n## `{group}`\n\n")
        for pkg in packages:
            description = ctx.package_descriptions.get(f"{group}/{pkg}", "")
            if description:
                config_parts.append(f"  {pkg} = {description}\n")
                doc_parts.append(f"- `{pkg}` — {description}\n")
            else:
                config_parts.append(f"  {pkg}\n")
                doc_parts.append(f"- `{pkg}`\n")
        config_parts.append("}\n")
        wrote_group = True
    # `dotfmt` owns the `=` column, so this only has to emit the entries.
    return formatted("".join(config_parts), ctx.packages_config), "".join(doc_parts)


def replace_package_file(content, destination, label):
    if os.path.isfile(destination):
        with open(destination, encoding="utf-8") as handle:
            if handle.read() == content:
                return 0
    with open(destination, "w", encoding="utf-8") as handle:
        handle.write(content)
    log(f"updated {label}")
    return 1


def sync_packages(ctx):
    validate_package_names(ctx)
    config, doc = render_packages(ctx)
    changed = replace_package_file(config, ctx.packages_config, "config/packages.dotfile")
    changed += replace_package_file(doc, ctx.packages_doc, "PACKAGES.md")
    return changed


def cmd_packages(ctx):
    load_package_metadata(ctx)
    if sync_packages(ctx) == 0:
        log("packages are current")
