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
ends up in terminal scrollback and in agent transcripts.

Encrypted files are checked for their invariants and then skipped, since
ciphertext is the protected form.

### Where it runs

`pre-commit` scans staged content, as the last step, after the regeneration
steps that stage generated files. `pre-push` scans every blob added by the
commits being pushed, which catches a value that was committed and then edited
out before the push, exactly the case that happened, and which still fires when
`--no-verify` skipped the commit hook.

There is no CI. Both hooks fail closed instead: if `scripts/.venv` is missing
they refuse rather than skipping, since a scanner that silently disables itself
is worse than none. Run `./setup.sh` on a new machine before committing.

`--no-canaries` exists for any context that must not hold the value list.

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

## Keys

Encryption is age, driven by sops. Every machine holds its own identity and
nobody shares a private key.

### The machine identity

`dotfile secret init` writes one to `~/.config/dotfile/age/keys.txt`, mode 600
in a 700 directory, and prints only the public half. The private key is never
printed, never copied, and never leaves the machine that generated it.

It sits in the dotfile state directory rather than at sops' own default path so
that one location holds everything dotfile owns and the path is the same on
Linux and macOS. `SOPS_AGE_KEY_FILE` is exported from
`shared/zsh/conf.d/10-env.zsh` so a bare `sops` call finds it, and every
`dotfile secret` command sets it explicitly so hooks and scripts do not depend
on the shell. `doctor` reports an identity left at sops' default path, since
that is the confusing failure to catch early.

### keys.dotfile and .sops.yaml

`keys.dotfile` is the source of truth: a label and a public key per recipient.
`.sops.yaml` is generated from it, sorted, and staged by `pre-commit`, in the
same way `packages.dotfile` generates `PACKAGES.md`. Never edit `.sops.yaml`
directly; `doctor` reports the drift if you do.

    recipients {
      archpc = age1...
      recovery = age1...
    }

Every recipient can read every encrypted file. Per-path scoping is a sops
feature worth reaching for only when there is something one machine should not
read, and on a workstation-only repository there is nothing yet.

### Adding a machine

On the new machine, `dotfile secret init` and copy the printed public key. On a
machine that already has access, `dotfile secret enroll <label> <key>`, then
`dotfile secret sync --rewrap` to re-wrap the existing files, then commit. The
new machine pulls and can read them.

The handshake needs a machine that already has access, by construction: no
shared secret ever travels, and there is no path by which a stranger with the
repository can grant themselves one.

### The recovery key

Because adding a machine needs an existing machine, a single dead disk would
otherwise be permanent loss. One recipient labelled `recovery` fixes that, and
`doctor` fails until it exists.

Generate it somewhere temporary, put the private key in a password manager, and
destroy the file:

    age-keygen -o /tmp/recovery.txt
    age-keygen -y /tmp/recovery.txt
    dotfile secret enroll recovery <the public key>
    shred -u /tmp/recovery.txt

The private half must never land in this repository, in the state directory, or
in a terminal that is being recorded.

### Revoking

`dotfile secret revoke <label>` drops a recipient and regenerates `.sops.yaml`;
`dotfile secret sync --rewrap` re-wraps every encrypted file so the removed key
can no longer open new copies. That is all it does. The old key already read
whatever it read, and a copy of the repository from before the revoke is still
readable with it, so a retired or lost machine means rotating the secrets
themselves.

### doctor

`dotfile secret doctor` checks the whole chain: age and sops on PATH, the
identity present and correctly moded, this machine enrolled, a recovery
recipient present, `.sops.yaml` matching `keys.dotfile`, every encrypted file
decryptable here, the canaries file present and 600, both hooks active, and no
stray identity where sops would find it first. It exits non-zero when any of
those fail, so it works as a post-setup check on a new machine.

## The vault

Secrets are decrypted to their destination as real files. They are never
symlinked, because a symlink into the repository requires the plaintext to live
in the working tree, where one `git add -A` ends the whole exercise. This is
the one place the repository gives up its edit-in-place model, and everything
below is the cost of that trade.

### Naming

A package containing a `.secret` marker is materialised rather than linked, and
nothing inside it may be unencrypted. A single `*.enc` file inside an ordinary
public package is materialised on its own, leaving the rest of the package
linked as usual.

The suffix also selects how sops treats the file:

    config.enc        binary, whole file opaque    ->  ~/.ssh/config
    facts.enc.yaml    structured, keys readable    ->  facts.yaml

Use the plain `.enc` form for anything opaque: ssh configs, private keys,
WireGuard. Use the `.enc.<ext>` form when the keys are worth reading in a diff
and only the values are secret, which is what phase 3 builds on.

### States

`dotfile secret status` reports one state per destination.

    current     matches the repository
    wrote       decrypted and written
    remoded     content matched, permissions corrected
    absent      not present on this machine
    sealed      encrypted here, no age identity to open it
    drifted     exists but differs from the repository
    failed      no recipient on this machine can decrypt it
    plaintext   unencrypted file inside a .secret package

`drifted`, `failed` and `plaintext` are blocking: they make `apply`, `clean`
and `link` exit non-zero. `sealed` never blocks, so an unenrolled machine still
links everything public.

### Local edits are never discarded silently

The symlink model makes an edit to a destination an edit to the repository.
Materialised files break that, so a destination that differs from its encrypted
source is reported as `drifted` and left alone. `apply` will not overwrite it,
`clean` will not delete it, and `link` fails rather than proceeding.

`dotfile secret edit <path>` is the way through: it opens the decrypted content
in `$EDITOR`, re-encrypts on save, and re-applies. Quitting without a change is
not an error. `apply --force` is the other way, and it discards the local edit.

### Permissions

Materialised files are 0600, written through `os.open` with the mode set at
creation so the content never exists at a laxer mode, even briefly. Directories
created along the way are 0700.

Existing directories are left alone, with one exception: the destination of a
`.secret` package is corrected to 0700, because ssh refuses to use a key under
a group readable directory and the whole point of the marker is that everything
below it is private. The protected paths that `link` will never fold — `$HOME`,
`~/.config`, `~/.local` and friends — are never chmodded.

### Working with it

    dotfile secret add ~/.ssh/config --pkg ssh
    dotfile secret edit shared/ssh/config.enc
    dotfile secret status
    dotfile secret apply
    dotfile secret clean

`add` encrypts a live file into the repository and leaves the original where it
is, at 0600 — the live file is already the materialised form, so there is
nothing to move and nothing to link. It writes the `.secret` marker when it
creates a new package, and maps the package to the source directory in
`targets` when that is not already the default.

The plaintext never passes through the repository: sops reads the live path
directly and the ciphertext is written to the repository path, with `--config`
pointing at `.sops.yaml` so the recipients resolve from outside the tree.

`edit` accepts either the repository path or the destination path. `clean`
removes materialised files, which is what to run before handing a machine on.
`dotfile link` runs `apply` as its last phase, so the normal setup path needs
none of these directly.

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
