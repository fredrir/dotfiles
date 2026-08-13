import os
import re

from tools.core import blocks
from tools.dotfile.state import die

AGE_KEY = re.compile(r"^age1[02-9ac-hj-np-z]{58}$")
LABEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
RECOVERY = "recovery"

STRUCTURE_ERRORS = {
    blocks.UNEXPECTED_CLOSE: "unexpected }",
    blocks.NESTED: "nested block",
    blocks.OUTSIDE: "line outside a 'recipients {' block",
}


def is_recovery(label):
    return label.lower().startswith(RECOVERY)


def recovery_labels(recipients):
    return [label for label in sorted(recipients) if is_recovery(label)]


def keys_file(ctx):
    return os.path.join(ctx.root, "keys.dotfile")


def sops_file(ctx):
    return os.path.join(ctx.root, ".sops.yaml")


def load_recipients(ctx):
    path = keys_file(ctx)
    found = {}
    if not os.path.isfile(path):
        return found
    try:
        entries = blocks.read(path)
    except blocks.BlockError as error:
        if error.kind == blocks.UNTERMINATED:
            die("keys.dotfile: unterminated block")
        die(f"keys.dotfile:{error.number}: {STRUCTURE_ERRORS[error.kind]}")
        return found
    for entry in entries:
        if entry.opens:
            if entry.block != "recipients":
                die(f"keys.dotfile:{entry.number}: unknown block '{entry.block}'")
            continue
        if "=" not in entry.text:
            die(f"keys.dotfile:{entry.number}: expected <label> = <age public key>")
        label, key = entry.split("=")
        if not LABEL.match(label):
            die(f"keys.dotfile:{entry.number}: bad label '{label}'")
        if not AGE_KEY.match(key):
            die(f"keys.dotfile:{entry.number}: '{label}' is not an age public key")
        if label in found:
            die(f"keys.dotfile:{entry.number}: duplicate label '{label}'")
        found[label] = key
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
