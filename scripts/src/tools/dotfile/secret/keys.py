import os
import re

from tools.dotfile.state import die, trim

AGE_KEY = re.compile(r"^age1[02-9ac-hj-np-z]{58}$")
LABEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
RECOVERY = "recovery"


def keys_file(ctx):
    return os.path.join(ctx.root, "keys.dotfile")


def sops_file(ctx):
    return os.path.join(ctx.root, ".sops.yaml")


def load_recipients(ctx):
    path = keys_file(ctx)
    found = {}
    if not os.path.isfile(path):
        return found
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    block = ""
    number = 0
    for raw in lines:
        number += 1
        line = trim(raw.split("#", 1)[0])
        if not line:
            continue
        if line == "}":
            if not block:
                die(f"keys.dotfile:{number}: unexpected }}")
            block = ""
            continue
        if line.endswith("{"):
            if block:
                die(f"keys.dotfile:{number}: nested block")
            block = trim(line[:-1])
            if block != "recipients":
                die(f"keys.dotfile:{number}: unknown block '{block}'")
            continue
        if not block:
            die(f"keys.dotfile:{number}: line outside a 'recipients {{' block")
        if "=" not in line:
            die(f"keys.dotfile:{number}: expected <label> = <age public key>")
        label, _, key = line.partition("=")
        label, key = trim(label), trim(key)
        if not LABEL.match(label):
            die(f"keys.dotfile:{number}: bad label '{label}'")
        if not AGE_KEY.match(key):
            die(f"keys.dotfile:{number}: '{label}' is not an age public key")
        if label in found:
            die(f"keys.dotfile:{number}: duplicate label '{label}'")
        found[label] = key
    if block:
        die("keys.dotfile: unterminated block")
    return found


def keys_document(recipients):
    body = "".join(f"  {label} = {recipients[label]}\n" for label in sorted(recipients))
    return f"recipients {{\n{body}}}\n"


def sops_document(recipients):
    if not recipients:
        return ""
    joined = ",".join(recipients[label] for label in sorted(recipients))
    return f"creation_rules:\n  - age: {joined}\n"


def write_if_changed(path, content):
    previous = ""
    if os.path.isfile(path):
        with open(path, encoding="utf-8") as handle:
            previous = handle.read()
    if previous == content:
        return False
    if not content:
        if os.path.isfile(path):
            os.remove(path)
            return True
        return False
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(content)
    return True


def save_recipients(ctx, recipients):
    write_if_changed(keys_file(ctx), keys_document(recipients))
    return write_if_changed(sops_file(ctx), sops_document(recipients))


def sops_drifted(ctx, recipients):
    path = sops_file(ctx)
    expected = sops_document(recipients)
    if not os.path.isfile(path):
        return bool(expected)
    with open(path, encoding="utf-8") as handle:
        return handle.read() != expected


def label_for(recipients, key):
    for label, value in recipients.items():
        if value == key:
            return label
    return ""
