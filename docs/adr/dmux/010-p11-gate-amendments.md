# ADR 010: P11 gate amendments — case 17 split, canary enablement, reader test, wrapper verbs

Status: accepted (P11; amendments made before the canary starts)
Date: 2026-08-18
Owner: root integrator
Plan refs: §8.3, §17, §20.2 case 17, §21 steps 7/9, §22

Three items in the P11 gate were underspecified or self-contradictory in a way
that would have surfaced as a *failing* gate rather than as a spec question, and
a fourth — §17's rule for the legacy wrappers — is followable to the letter and
wrong in the hand. Each is corrected here, before the canary begins, with the
reasoning recorded.

## 1. Acceptance case 17 is split into 17a and 17b

### What the plan said

> 17. USB absent before selection creates tmux.

### What the code does

`RouteState::PositivelyAbsent` (`scripts/rust/crates/dmux/src/policy.rs:22-24`)
is produced in exactly one situation: `usb_routes.is_empty()` at
`src/gui_cli.rs:2324-2325` — that is, **no enrolled USB route row exists** for
the target host. An unplugged cable does not produce it. An unplugged cable
produces `WezRouteProbe::TransportFailed` → `RouteState::ProbeFailed`, and
`decide_backend` then **refuses** rather than creating tmux.

### Why the code is right and the case text is wrong

This is not a gap; it is §8.3 being implemented correctly and case 17 being
written loosely. §8.3 is explicit:

> Only a positively observed pre-selection `route_absent`/`usb_link_down` (no
> enrolled USB route or an authoritative link-state signal) makes USB
> ineligible and permits automatic tmux selection. DNS failure, refusal/reset,
> and timeout are not proof of "unwired"; during eligibility preflight they
> return route unavailable rather than choose tmux.

An unplugged cable observed through a TCP/SSH probe is indistinguishable from a
transient network failure, a wedged remote sshd, or a routing change. Treating
it as "unwired" and silently creating tmux is precisely the silent backend
substitution §1 and §22 forbid ("Once Wez has been selected, an authentication,
protocol, compatibility, or mutation failure never silently creates a tmux
Space"). Case 17 as written asks for the behavior the rest of the document
prohibits.

### The correction

Case 17 becomes two cases with distinct expected outcomes:

- **17a** — no enrolled USB route → positive `route_absent` → automatic tmux.
- **17b** — enrolled USB route present, eligibility probe fails → refuse,
  create neither backend.

Both are mandatory. 17b is the stronger assertion and is the one that protects
the no-silent-fallback property; it did not previously exist as a numbered case
at all, so splitting *adds* a gate rather than removing one.

### What this deliberately does not do

It does not enroll explicit USB and Tailscale route rows in order to make a
`PositivelyAbsent` observation reachable on the real hosts. Enrolling addresses
to manufacture a passing case would be gate-fitting. Route topology is changed
only where cases 16 and 18–20 independently require distinguishable routes; that
question is decided on its own evidence, not on case 17's behalf.

## 2. The §21 canary/flip circular gate

§21 step 7 requires a 24–48-hour **auto-Wez** canary, which requires automatic
Wez policy to be active. §21 step 9 forbids flipping automatic policy until the
full P11 gate passes — and §18's P11 row puts the canary inside that gate. As
written: flip ← gate ← canary ← flip.

The escape already exists in the code and was simply never named in §21:
`DMUX_WEZ_FIRST=1` is a **host-scoped, process-scoped** opt-in read at config
evaluation time (28 read sites across 18 files; see
`shared/wezterm/wez/domains/init.lua:112-126` for the rationale). It makes one
host's GUI an attach-only client of the managed mux, which is what makes the
automatic path select Wez there — without changing any default anywhere else.

§21 is amended to say so explicitly:

- step 7's canary runs under `DMUX_WEZ_FIRST=1` on the canary host;
- step 9's "flip globally" means changing the default that applies when
  `DMUX_WEZ_FIRST` is **unset**, plus shipping the emergency opt-out.

A host already canarying sees no behavior change at the flip. The flag becomes
redundant rather than removed; it is retained for one release alongside the
opt-out.

## 3. The "fresh-context reader test" is defined

§18 and §22 both gate P11 on a "fresh-context reader test". The phrase occurs
exactly twice in the plan and is defined nowhere — not in §20.1's test layers,
not as a numbered case, and not in §19's QA/reader role (whose write scope is
`tests/{black_box,fault}/**` only). The nearest precedent in this repository is
`docs/dotfile-language.md`, which describes "fresh-reader tests that correctly
answer graph, manager, mapping, variant, lock, and mutation questions using this
document alone".

Defined by analogy, and recorded here as the binding definition:

> A reader with **no prior context on this project**, given only
> `docs/dmux-wezterm-first-plan.md` and `docs/adr/dmux/**`, answers a fixed
> question set covering: the human reference grammar (§6.2), the automatic
> creation decision table (§8.3) including which failures refuse rather than
> fall back, registry lock ordering (§10.1), the bridge origin kinds and what
> makes a request unauthorized (§13.2), and cold-recovery eligibility (§15.3).
> The questions and the reader's answers are checked in as an artifact, and the
> test passes when every answer is correct **and** each is traceable to a
> section the reader can cite.

The reader must not be the root integrator, which wrote the plan. §19.2's "at
W7 specialists stop" does not forbid this: a fresh reader is not an editing
specialist and holds no path ownership.

## 4. §17 permits a verb allowlist that is mechanically verified

### What the plan said

> No wrapper maintains a subcommand allowlist, parses backend flags, or
> contains backend logic.

### What the literal reading produces

Without a list the wrapper has one rule for a lone bare word, and P11 took the
create rule: `ssa ls` becomes `dmux --host archie new ls` and creates a Space
called "ls" instead of listing. So does every other one-word verb — `ssa
detach`, `ssa doctor`, `ssa keys`, `ssa help` — because nothing in the argv
distinguishes a name from a verb, and all of those names are legal under §6.2's
grammar, so none of them is refused. §17 asks the same wrapper to be
create-or-connect for a name *and* passthrough for everything else; deleting
the list does not resolve that, it just decides it silently in one direction.

### Why the list is not the hazard §17 is aiming at

The hazard is drift, and the deleted test said so in its own comment:

> The case list is maintained by hand, so every verb the CLI grows has to show
> up here: `ssa detach` must detach, not create a session named "detach".

That comment is half wrong and worth correcting rather than quoting approvingly.
`ssa detach` does not detach and never could: `disconnect` acts on the invoking
local client and rejects `--host` outright (`main.rs:1209-1213`), which the
wrapper always supplies. Both spellings exit 2. That is the right outcome —
there is nothing a host-scoped detach could mean — so the allowlist's value here
is narrower than the comment claims: `ssa detach` fails with a usage error
instead of silently creating a Space named "detach". The second half of the
comment is the real requirement, and it is the one that had already been broken. At the time the list was deleted it named 14 verbs
while the CLI exposed 22: `disconnect`, `recovery`, `group`, `split`,
`context`, `repair`, `ssh`, and `host` had all been added without it, so `ssa
host` created a Space named "host". A hand-maintained list is a promise that
nothing enforces, and this one had already been broken.

Asking the binary at each invocation is the other way to close the drift, and
is rejected: the wrapper sits in an interactive shell's hot path, where a
process spawn per `ssa foo` buys nothing a build-time check does not.

### The correction

The list stays, and equality with the CLI becomes a test rather than a habit.
`the_wrapper_verb_allowlist_matches_the_cli`
(`scripts/rust/crates/dmux/tests/cli.rs`) derives the authoritative subcommand
and alias names from the binary's own clap command tree, excludes the commands
clap itself marks hidden, and fails naming each verb that is missing from the
wrapper or stale in it. A verb the CLI grows cannot reach a release as a Space
constructor.

Both halves of that comparison are evaluated, not parsed, because an adversarial
pass defeated the parsing version of each. Deriving the CLI side from
`--completions`/`--help` text missed `#[command(alias = ...)]` entirely — a
non-visible alias appears in neither, so `dmux lst` worked as `ls` while the
wrapper turned `ssa lst` into a Space, with both suites green. Reading the zsh
array as text was worse: quoting the elements, interleaving a comment, using
line-continuations, or splitting the assignment each broke the parse without
changing one byte of behaviour, and a one-line comment that merely mentioned the
array name captured the parse outright, hiding three real verbs. The array is
therefore declared at file scope and read by sourcing it in `zsh -f`.

§17's sentence is amended to permit exactly that and nothing more: a verb
allowlist mechanically verified against the CLI, still no backend flags and no
backend logic. The create spelling in the same paragraph stays `new NAME`;
§2.11 removes `con --create` after one compatibility release, so the old
wrapper's `con -A NAME` is not what is being restored.

### What this deliberately does not do

The wrapper does not interpret the verbs it knows. A listed verb is forwarded
verbatim — the list decides *not to create*, it never selects a backend,
rewrites a flag, or supplies a default. Nor does the list adjudicate a Space
genuinely named after a verb. Spell the verb: `ssa new ls` creates-or-connects
one named "ls". §7.4's `con --name` escape is not the answer today — it is
connect-only, so it cannot make the Space, and it is gated on `DMUX_WEZ_FIRST`
until the cutover, so in a default shell it exits 2 (`main.rs:605`).

## Consequences

- Plan §20.2 case 17 and §21 steps 7 and 9 are amended in the same change as
  this ADR.
- The 46-case count is unchanged; case 17 carries two lettered sub-cases.
- No gate is weakened. Case 17b is a new assertion, the canary resolution
  removes an impossibility rather than a requirement, and the reader test moves
  from undefined to testable.
- Plan §17's wrapper sentence is amended in the same change as this ADR, and
  the wrapper's allowlist gains the first enforcement it has ever had.
