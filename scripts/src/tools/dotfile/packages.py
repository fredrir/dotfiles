import os

from tools.dotfile.state import die, log, manifest_groups, sorted_entries, trim

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


def load_package_metadata(ctx):
    ctx.package_descriptions = {}
    if not os.path.isfile(ctx.packages_config):
        return
    group = ""
    number = 0
    with open(ctx.packages_config, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    for raw in lines:
        number += 1
        line = trim(raw)
        if not line:
            continue
        if line == "}":
            if not group:
                die(f"packages.dotfile:{number}: unexpected }}")
            group = ""
            continue
        if line.endswith(" {"):
            if group:
                die(f"packages.dotfile:{number}: nested group")
            group = trim(line[:-2])
            if not group:
                die(f"packages.dotfile:{number}: empty group")
            if set(group) - GROUP_CHARS:
                die(f"packages.dotfile:{number}: invalid group: {group}")
            continue
        if not group:
            die(f"packages.dotfile:{number}: package outside a group")
        if " = " in line:
            name = trim(line.split(" = ", 1)[0])
            description = trim(line.split(" = ", 1)[1])
        else:
            name = line
            description = ""
        if not name:
            die(f"packages.dotfile:{number}: empty package")
        if set(name) - PACKAGE_CHARS:
            die(f"packages.dotfile:{number}: invalid package: {name}")
        key = f"{group}/{name}"
        if key in ctx.package_descriptions:
            die(f"packages.dotfile:{number}: duplicate package: {key}")
        ctx.package_descriptions[key] = description
    if group:
        die(f"packages.dotfile:{number}: missing }} for {group}")


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
    return "".join(config_parts), "".join(doc_parts)


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
    changed = replace_package_file(config, ctx.packages_config, "packages.dotfile")
    changed += replace_package_file(doc, ctx.packages_doc, "PACKAGES.md")
    return changed


def cmd_packages(ctx):
    load_package_metadata(ctx)
    if sync_packages(ctx) == 0:
        log("packages are current")
