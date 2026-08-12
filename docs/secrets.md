# secrets

This repository is public and its history is permanent. Anything committed is
world readable from the moment it is pushed, and stays readable after it is
removed from `HEAD`: clones keep it, forks keep it, and GitHub serves
unreachable objects by hash for a long time. Deleting a value from the working
tree is a tidy-up, never a containment measure.

The plan has five phases. This document covers phase 0, which is containment
and detection and involves no cryptography. Encryption arrives in phase 2, and
the value templating layer in phase 3.

## What went wrong once

An agent skill file described a production host by name and origin address. It
lived in twelve commits before being edited out. It was not a token, not a key,
and nothing in `.gitignore` was written with it in mind, because the category
did not exist when the ignore rules were written.

Two things follow. Encryption would not have caught it, because nobody thought
of that file as secret. And it will recur, because every agent skill, every
note about the infrastructure, and every transcript is a place where an address
can land. Detection has to be independent of remembering to encrypt.

## Three classes

**Secret.** Material that grants access: private keys, signing keys, backup
repository keys, WireGuard configuration, licence keys. Encrypted at rest,
never present in the working tree as plaintext. Phase 2.

**Private.** Values that are not credentials but should not be published:
origin addresses behind a proxy, machine identifiers, hardware serials, MAC
addresses, personal addresses. These live in one place and are referenced by
name from otherwise public configs. Phase 3.

**Public.** Everything else, which is nearly all of this repository, and which
stays readable and reviewable in diffs. Encrypting a config because one line
in it names a host is a bad trade; the line becomes a reference instead.

## What to track

Track a secret only when it is long lived, painful to regenerate, and needed on
more than one machine. SSH keys, signing keys, backup repository keys and
WireGuard qualify.

Credentials that refresh do not. `gh`'s `hosts.yml` holds an OAuth token that
rotates; tracking it buys a commit per rotation and nothing else, when
`gh auth login` regenerates it in seconds. Those stay in `.gitignore`.

## dotfile secret scan

Three tiers, run together, cheapest first.

**Pattern.** Token shapes and key blocks: GitHub, GitLab, npm, AWS, Slack,
JWTs, age identities, and any PEM private key block, plus assignments to names
like `api_key` or `password`. The pattern set lives in
`scripts/src/tools/core/patterns.py` and is shared with the transcript
archiver, so there is one definition of what a secret looks like.

**Canary.** Literal private values, read from `~/.config/dotfile/canaries`,
matched case insensitively anywhere in the scanned content. This is the tier
that catches an origin address in a skill file, and no general purpose scanner
can do it, because only this machine knows that string matters.

**Invariant.** Structural rules that hold whether or not anything looks like a
secret: a file named `*.enc` must carry sops metadata, a package containing a
`.secret` marker must contain nothing unencrypted, and a key shaped filename
(`id_ed25519`, `*.pem`, `.env`, and friends) must not appear outside such a
package. These rules do nothing yet and become load bearing in phase 2.

Findings are reported by label and location. The matched text is masked, and a
canary's value is never printed at all, only its label, because scanner output
ends up in terminal scrollback, in CI logs, and in agent transcripts.

Encrypted files are checked for their invariants and then skipped, since
ciphertext is the protected form.

### Where it runs

`pre-commit` scans staged content. `pre-push` scans every blob added by the
commits being pushed, which catches a value that was committed and then edited
out before the push, exactly the case that happened. CI scans the tree on every
push and pull request with `--no-canaries`, because the canary list must never
leave this machine.

Both hooks fail closed. If `scripts/.venv` is missing they refuse rather than
skipping, since a scanner that silently disables itself is worse than none.
Run `./setup.sh` on a new machine before committing.

### Canaries

One value per line in `~/.config/dotfile/canaries`, optionally labelled:

    parser-origin = 203.0.113.77
    desktop-mac = aa:bb:cc:dd:ee:ff
    203.0.113.90

The file lives in the state directory beside `profile` and `overrides`, outside
the repository, so it can never be committed. It should be mode 600; the scan
says so if it is not. Values shorter than six characters are rejected, because
a short string matches everywhere and trains you to ignore the output.

Matching is literal. A MAC written both as `aa:bb:cc:dd:ee:ff` and
`aa-bb-cc-dd-ee-ff` needs both forms listed.

In phase 3 this file becomes the plaintext view of the encrypted facts map, and
the same values feed the templates and the transcript redactor.

### scan.dotfile

Pattern findings can be allowed per path, optionally narrowed to one label:

    allow {
      scripts/tests/transcript/test_redact.py
      shared/obsidian/plugins/** value
    }

A bare path allows every label for that path. A trailing label allows only
that one, so an allowed file stays sensitive to everything else. Globs use
shell wildcards and `*` crosses directory separators.

Canary and invariant findings are never allowed. A false positive there means
the rule is wrong, not the file.

The current list is short and deliberately so: two vendored files whose
minified or documentation content trips the value pattern, and the two test
files that carry fixtures by design. Widening it to whole trees would have
allowed exactly the directory the address leaked from.

## The vault is a second surface

`~/Documents/main` is an Obsidian vault kept in Obsidian Sync, and the
transcript pipeline archives agent sessions into it. Agent transcripts discuss
infrastructure constantly, which is how the address reached a skill file in the
first place. The same values that guard the repository should redact on the way
into the vault; the transcript redactor already shares the pattern set, and
gains the canary values in phase 3.

## Assumptions worth stating

From phase 1 the age identity sits unencrypted on disk, so full disk encryption
is load bearing: physical access to any machine would otherwise compromise
everything ever encrypted, on every machine. Moving the identity to a hardware
token removes that assumption and is the last item on the roadmap.

Encryption is not revocation. Removing a machine's key and re-wrapping the
files does not un-see the plaintext that key already read. Retiring a machine
means rotating the underlying secrets.

This repository is a workstation repository. Nothing here is provisioned to a
server, and server side secrets are out of scope.
