# ADR 010: P11 gate amendments — case 17 split, canary enablement, reader test

Status: accepted (P11; amendments made before the canary starts)
Date: 2026-08-18
Owner: root integrator
Plan refs: §8.3, §20.2 case 17, §21 steps 7/9, §22

Three items in the P11 gate were underspecified or self-contradictory in a way
that would have surfaced as a *failing* gate rather than as a spec question.
Each is corrected here, before the canary begins, with the reasoning recorded.

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

## Consequences

- Plan §20.2 case 17 and §21 steps 7 and 9 are amended in the same change as
  this ADR.
- The 46-case count is unchanged; case 17 carries two lettered sub-cases.
- No gate is weakened. Case 17b is a new assertion, the canary resolution
  removes an impossibility rather than a requirement, and the reader test moves
  from undefined to testable.
