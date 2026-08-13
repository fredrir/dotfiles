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

**Secret.** Material that grants access and that every machine must hold the
same copy of: an apt signing key, a licence key, a token that cannot be
duplicated. Encrypted at rest, never present in the working tree as plaintext.
Phase 2. The list is shorter than it looks, and the section below explains why.

**Private.** Values that are not credentials but should not be published:
origin addresses behind a proxy, machine identifiers, hardware serials, MAC
addresses, personal addresses. These live in one place and are referenced by
name from otherwise public configs. Phase 3.

**Public.** Everything else, which is nearly all of this repository, and which
stays readable and reviewable in diffs. Encrypting a config because one line
in it names a host is a bad trade; the line becomes a reference instead.

## What to track

Four tests, all of which have to pass. A secret belongs here only when it is
long lived, painful to regenerate, needed on more than one machine, and **cannot
simply be different on each machine**.

That fourth test is the one that does the work, and it disqualifies most of
what looks like an obvious candidate.

Credentials that refresh fail the first test. `gh`'s `hosts.yml` holds an OAuth
token that rotates; tracking it buys a commit per rotation and nothing else,
when `gh auth login` regenerates it in seconds. Those stay in `.gitignore`.

### An application directory holds three things

Preferences you authored, identity the application generated, and state it
maintains while running. Only the first belongs here. The second regenerates —
re-pair, re-authenticate — and the third churns.

Sort by file, never by filtering inside one. An application that keeps the
three apart is trackable: Sunshine's `apps.json` and `sunshine.conf` are pure
preference, while `credentials/`, `sunshine_state.json` and the log are not, so
two files are tracked and three are ignored.

An application that mixes them into one file is not trackable at all, and the
reason is the restore rather than the secret. Everything here installs by
overwriting its destination. A filtered copy of Moonlight's config would put
back the preferences and destroy the client key, the certificate and every
paired host along with them. Filtering would only be safe with a format-aware
merge that writes back the managed keys and leaves the rest — a third install
mode, to preserve settings that take two minutes to re-enter.

The same applies to a macOS `defaults` domain: restoring means `defaults
import`, which replaces the whole domain, so a hand-pruned plist deletes
whatever it omits, licence keys included.

Those files are listed in `.gitignore` so the decision is enforced rather than
remembered. A Moonlight config would also be caught by the scanner on its way
in, since the key is stored as PEM.

### SSH keys do not belong here

They fail the fourth test, and it is worth writing down why, because they look
like the strongest candidate right up until you think about it.

Give each machine its own keypair instead. Add each *public* key to GitHub,
which accepts many, and to each server's `authorized_keys`, which also accepts
many. Then no private key ever leaves the machine that generated it, nothing
key-shaped is published anywhere in any form, a lost machine costs you one
public key deleted from a few places rather than a rotation of everything, and
the server logs tell you which machine connected.

The cost is enrolling a new machine's public key in a handful of places, once,
at setup. That is an afternoon every few years.

### Why a public repository raises the bar

Everything here is world readable the moment it is pushed, and permanently.
Three consequences shape the rule above.

Publication cannot be undone, and rotation does not reach backwards. Rotating a
key leaves the old encrypted copy in history, still readable by the same age
identity. A future compromise of `~/.config/dotfile/age/keys.txt` therefore
exposes every secret ever committed, including the ones already rotated, and an
attacker with a clone already holds all the ciphertext they will ever need.

An offline attack has no ceiling: no rate limit, no lockout, no revocation, and
nothing that tells you it is being attempted.

Ciphertext archived today can be decrypted later. age uses X25519, which a
sufficiently large quantum computer breaks. Weight this lowest — nobody is
stockpiling these files — but it costs nothing to keep long lived key material
out of a public repository, so keep it out.

None of this is a statement about the cryptography, which is sound. sops and
age do all of it; the code in this repository only decides which files to hand
them, where to put the output, and what mode to set. It is a statement about
publishing ciphertext irrevocably, which is a deployment choice rather than a
code quality one.

If something genuinely must be synchronised and genuinely cannot be
per-machine, a private repository is the better home: an attacker then needs
both a GitHub credential and the age identity, instead of the identity alone.

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

In phase 3 this file becomes the plaintext companion to the encrypted
`vars.enc.yaml`, and the same values feed the templates and the transcript
redactor.

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

    apt-signing.asc.enc   binary, whole file opaque   ->  apt-signing.asc
    vars.enc.yaml         structured, keys readable   ->  vars.yaml

Use the plain `.enc` form for anything opaque: signing keys, licence files.
Use the `.enc.<ext>` form when the keys are worth reading in a diff and only
the values are secret, which is what phase 3 builds on.

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
`.secret` package is corrected to 0700, since the whole point of the marker is
that everything below it is private. The protected paths that `link` will never
fold — `$HOME`, `~/.config`, `~/.local` and friends — are never chmodded.

### Working with it

    dotfile secret add ~/.local/share/keyring/apt-signing.asc --pkg apt
    dotfile secret edit shared/apt/apt-signing.asc.enc
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

## Vars

Encryption answers "this whole file is secret". Most of what leaks is not a
whole file: it is one address inside a config that is otherwise entirely public
and worth reading in a diff. Encrypting `40-aliases.zsh` because one line names
a host trades a reviewable config for an opaque blob, and you would do it a
dozen more times.

So the second layer keeps the config public and moves the value out of it.

    Host parser
      HostName {{ hosts.parser.origin }}
      User {{ open.user }}

`vars.enc.yaml` at the repository root holds the values. It is encrypted
structurally rather than as a blob, so the key names stay readable in a diff
and only the values are ciphertext. Adding one is a single line you can
review; what it is worth stays hidden.

Templates carry a `.tmpl` suffix and render to the name without it, so
`shared/ssh/config.tmpl` becomes `~/.ssh/config`. A template inside a `.secret`
package is allowed, because a template holds placeholders rather than values —
and if one ever holds a real value instead, the canary tier catches it.

An ssh *config* is a fair template: its shape is public and only the addresses
are not. An ssh *key* is still not tracked at all, per the rule above, and the
two decisions are unrelated. Anything genuinely per-machine belongs in the
`Include`d `~/.ssh/config.d`, which stays local for the same reason
`kitty/local.d` and `hypr/conf.d/local.conf` do.

Rendered files go through exactly the same path as decrypted ones: same drift
rule, same 0600, never symlinked, never written into the repository.

### Values

Nesting flattens to dotted names, and scalars render the way a config file
wants them: `port: 22` becomes `22`, `enabled: true` becomes `true`. Lists and
empty values are rejected, because a config file needs one string.

A template naming a var that does not exist is `unresolved`, which blocks
`apply` and `link` and reports the missing names. A half rendered config is
worse than no config, so nothing is written.

`dotfile secret vars` lists the names and which destination uses each, never
the values. `--unused` narrows it to the ones nothing references.

### Every var is a canary

The values feed the scanner automatically, labelled by their dotted name. Put
an address in `vars.enc.yaml` and any commit that carries it in plaintext is
refused from that moment on, with no separate list to maintain. This is the
control that would have caught the original leak, and it now arrives as a side
effect of using the value rather than as a thing to remember.

They also feed the transcript redactor, so an archived agent session that
mentions a host gets it stripped on the way into the vault.

`~/.config/dotfile/canaries` still works for values that are not template
inputs. The two sources are merged, and duplicates collapse.

### open.

Anything under a top level `open:` key renders like any other var but is not
canaried and is not redacted.

    open:
      user: someone
    hosts:
      parser:
        origin: 203.0.113.77

That distinction matters more than it looks. A var is matched literally
everywhere, so putting a common string like your username under `hosts:` would
refuse every commit that happens to contain it, including files that have
nothing to do with secrets. `open.` is for values that vary per machine and
belong in one place, but are not private: usernames, paths, display names.

Values shorter than six characters never become canaries either, since a short
string matches everywhere and trains you to ignore the output.

### Readable diffs

`.gitattributes` marks encrypted files `diff=sops`, and `setup.sh` points that
driver at `sops -d` with this machine's identity. `git diff` then shows the
decrypted content while the repository keeps the ciphertext, so a change to a
var reads as one changed line instead of a wall of re-encrypted base64.

`cachetextconv` is explicitly disabled. Enabling it would write the decrypted
output into `.git`, which is the one place this whole design exists to keep
plaintext out of. `doctor` fails if it is ever turned on.

The same file marks them `-merge`. sops re-encrypts a whole document on every
write with a fresh data key, so two machines editing one encrypted file produce
conflicting blobs that git cannot merge meaningfully; marking them unmergeable
turns a silent corruption into an honest conflict. Keep encrypted files small
and single-purpose for the same reason.

That non-determinism has a second consequence worth knowing: re-encrypting an
unchanged file still produces a different blob. This is why `secret edit`
treats sops exit 200 as success and leaves the file alone — quitting the editor
without a change writes nothing rather than churning the repository.

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
