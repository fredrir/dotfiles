export const meta = {
  name: 'dmux-epoch-integrity-review',
  description: 'Independent adversarial multi-agent review of epoch-verification integrity in the dmux crate',
  phases: [
    { title: 'Enumerate' },
    { title: 'Classify' },
    { title: 'Reach' },
    { title: 'Specialists' },
    { title: 'Repro' },
    { title: 'FixShape' },
    { title: 'Verify' },
    { title: 'Critic' },
    { title: 'Synthesis' },
  ],
}

const ROOT = '/Users/fredrir/dotfiles'
const CRATE = '/Users/fredrir/dotfiles/scripts/rust/crates/dmux'
const SCRATCH = '/private/tmp/claude-501/-Users-fredrir-dotfiles/12e80595-f7a4-4c00-a8da-74b02a86cabb/scratchpad'

const PREAMBLE = `
# Context
Repo: ${ROOT} (branch \`dmux\`, HEAD 493e92c). Crate under review: ${CRATE}.
Spec: ${ROOT}/docs/dmux-wezterm-first-plan.md (§-refs are to it). ADRs: ${ROOT}/docs/adr/dmux/**
(001 strict endpoint selection, 002 service/sentinel startup are the frozen mechanisms here).
Prebuilt binary: ${ROOT}/scripts/rust/target/debug/dmux. Live registry (READ-ONLY, copy it, never write it):
$HOME/.local/share/dmux/registry.sqlite3
Scratch dir for all temp files: ${SCRATCH}

# The defect class under review
\`registry.backend_server(instance)?.server_epoch\` returns \`Option<ServerEpoch>\`.
\`InventoryScope.expected_epoch\` (src/backend/mod.rs:121) is also \`Option<ServerEpoch>\`.
Both provider adapters check it as \`if let Some(expected)\` on the SCAN/READ path
(src/backend/wez.rs:1113, src/backend/tmux.rs:598), so \`None\` SKIPS VERIFICATION ENTIRELY and the
scan trusts whatever answers on the socket. Mutations DO require an epoch (wez.rs:1271,
tmux.rs:466) — the hole is reads, which feed the mutations.
An endpoint that is genuinely UNMANAGED (nothing registered; e.g. tmux first-contact discovery)
legitimately has no epoch. That is not a defect. Laundering a registry NULL for a MANAGED
instance into the same \`None\` is. You must state how you told the two apart at each site.

# Rules of evidence (hard)
- Ground EVERY claim in file:line. "Looks correct" / "should be fine" is not a finding.
- Prove reachability, don't assume it. Absence of a proof is not a pass.
- DO NOT MUTATE LIVE STATE. Never stop/restart/kickstart any mux server (plan §21: destructive).
  Never set DMUX_WEZ_FIRST. Never run \`git commit\`, \`git add\`, \`git checkout\`, \`git stash\`,
  \`git reset\`. Never write to $HOME/.local/share/dmux/. Do not edit files under ${CRATE}
  unless your task explicitly says to.
- Reading with \`sqlite3\` on a COPY is fine. Reading source, running \`cargo build/test\`, and
  \`git log/show/diff\` are fine.
- If you cannot verify something without a live canary or a reboot, say EXACTLY what remains
  untested and why. Do not infer a pass from code that reads correctly.
- A false lead disproved is as valuable as a bug found. Say plainly when a reported site is NOT
  a defect, with the reason.

# Prior findings — TEST these, do not assume them
Reported laundering sites: adopt_cli.rs:238, rm_cli.rs (~1138-1145), new_cli.rs:362-380,
gui_cli.rs:1430-1448, migrate_cli.rs:743, remote/agent.rs:1281, main.rs:1453.
Literal \`expected_epoch: None\` against a managed instance: space_cli.rs:1162, space_cli.rs:221-233.
Reported CLEAN: connect_cli.rs:1167, rm_cli.rs:1115, gui_lifecycle.rs:964, main.rs:1463, gui_cli.rs:1392.
Already fixed in 493e92c (ls_cli.rs ScanTarget::Managed/Unpublished + gui_lifecycle
validate_ready_descriptor no longer retryable on epoch mismatch) — VERIFY, do not trust.
Two open defects claimed in that fix: (a) ls_cli.rs ~779 \`Unpublished\` bypasses the operation
fence because \`ScanTarget::instance()\` returns None for it, conflating "unpublished" with
"recovering" and advising the operator to restart the mux — which would kill an in-flight
recovery; (b) ls_cli.rs ~1187 \`scan_error_code\`'s doc comment orphaned onto \`unpublished\`.
THE PRIOR LIST GREW TWICE AND WAS INCOMPLETE BOTH TIMES. Treat it as a starting point, never a
boundary. One item in the lists above is already known to be WRONG; find which.
A prior reviewer's summary to test rather than assume: "no managed-tmux read anywhere in the CLI
is epoch-verified."
`

const SITE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['sites', 'method', 'coverage_notes'],
  properties: {
    method: { type: 'string', description: 'Exact commands/greps you ran to be exhaustive' },
    coverage_notes: { type: 'string', description: 'What you could NOT cover and why' },
    sites: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['file', 'line', 'enclosing', 'snippet', 'epoch_source', 'initial_class', 'why'],
        properties: {
          file: { type: 'string', description: 'repo-relative path' },
          line: { type: 'integer' },
          enclosing: { type: 'string', description: 'enclosing fn / impl / test name' },
          snippet: { type: 'string', description: 'the 1-5 lines of code, verbatim' },
          epoch_source: {
            type: 'string',
            description: 'exactly where the epoch value comes from: Some(x) from where, or Option laundered from where, or literal None',
          },
          initial_class: {
            type: 'string',
            enum: ['verified', 'unverified-laundered', 'unverified-literal-none', 'intentionally-unmanaged', 'test-only', 'unreachable', 'unclear'],
          },
          why: { type: 'string', description: 'how you distinguished managed-laundering from genuinely-unmanaged, in one or two sentences' },
        },
      },
    },
  },
}

const REPORT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['summary', 'findings', 'untested', 'refuted'],
  properties: {
    summary: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['title', 'site', 'severity', 'reads_or_writes', 'reachability', 'evidence'],
        properties: {
          title: { type: 'string' },
          site: { type: 'string', description: 'file:line' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'info'] },
          reads_or_writes: { type: 'string', enum: ['read', 'writes-registry', 'writes-native-server', 'writes-pane-markers', 'writes-multiple', 'none'] },
          reachability: { type: 'string', description: 'the concrete invocation + state that reaches it, or why it is unreachable' },
          evidence: { type: 'string', description: 'file:line grounded proof' },
        },
      },
    },
    untested: { type: 'array', items: { type: 'string' }, description: 'exactly what remains unverified and why' },
    refuted: { type: 'array', items: { type: 'string' }, description: 'reported sites you determined are NOT defects, with reason' },
  },
}

const CLASSIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['site', 'verdict', 'reads_or_writes', 'rationale', 'evidence'],
  properties: {
    site: { type: 'string' },
    verdict: { type: 'string', enum: ['verified', 'unverified-defect', 'intentionally-unmanaged', 'unreachable', 'test-only', 'unclear'] },
    reads_or_writes: { type: 'string', enum: ['read', 'writes-registry', 'writes-native-server', 'writes-pane-markers', 'writes-multiple', 'none'] },
    rationale: { type: 'string' },
    evidence: { type: 'string' },
  },
}

const REACH_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['site', 'reachable', 'severity', 'invocation', 'preconditions', 'consequence', 'write_targets', 'evidence', 'confidence'],
  properties: {
    site: { type: 'string' },
    reachable: { type: 'string', enum: ['yes-null-epoch', 'yes-stale-epoch', 'yes-both', 'no', 'unproven'] },
    severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'info'] },
    invocation: { type: 'string', description: 'the exact CLI command / RPC that reaches it' },
    preconditions: { type: 'string', description: 'registry/server state required' },
    consequence: { type: 'string', description: 'observable effect' },
    write_targets: { type: 'string' },
    evidence: { type: 'string', description: 'call chain as file:line -> file:line -> ...' },
    confidence: { type: 'string', enum: ['proven-by-execution', 'proven-by-call-chain', 'plausible', 'speculative'] },
  },
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['refuted', 'reason', 'correction'],
  properties: {
    refuted: { type: 'boolean' },
    reason: { type: 'string' },
    correction: { type: 'string', description: 'if partially wrong, the corrected claim; else empty' },
  },
}

// ---------------------------------------------------------------------------
phase('Enumerate')
log('Phase 1: six independent sweeps for every epoch-carrying construction site')

const SWEEPS = [
  {
    label: 'sweep:scope-src',
    prompt: `${PREAMBLE}

## Your sweep: every \`InventoryScope\` construction in \`src/\`
Be EXHAUSTIVE. Do not stop at the reported list. Search at least these ways and reconcile them:
  rg -n 'InventoryScope' ${CRATE}/src
  rg -n 'expected_epoch' ${CRATE}/src
  rg -n 'InventoryScope\\s*\\{' -A4 ${CRATE}/src
  rg -n 'fn .*-> .*InventoryScope|fn .*InventoryScope' ${CRATE}/src
  also: helper constructors, \`..Default::default()\`, struct-update syntax, clones of an
  existing scope that then mutate \`expected_epoch\`, and scopes built inside closures/macros.
For EVERY construction site in src/ (including ones inside \`#[cfg(test)]\` modules in src/ —
mark those test-only), report the record. For each, trace where the epoch value came from and
say whether the instance is registry-managed at that point (look at the surrounding code:
did it just call \`backend_instance_for_backend\` / \`backend_server\` / hold a Space row?).
Return the structured list.`,
  },
  {
    label: 'sweep:scope-tests',
    prompt: `${PREAMBLE}

## Your sweep: every \`InventoryScope\` construction in \`${CRATE}/tests/\` and in test helpers
Search exhaustively (rg 'InventoryScope' and 'expected_epoch' under tests/, plus tests/*/util.rs
helper builders). Report each site. Crucially, answer these questions and put the answers in
\`coverage_notes\`:
 1. Do ANY tests construct a managed scope with \`expected_epoch: None\` and assert a SUCCESSFUL
    scan? Those are tests that ENCODE the bug as correct behaviour — list them by name; they are
    the reason this class survived review and they must be listed as findings material.
 2. Is there any test that asserts a managed read REFUSES when the epoch is NULL? If none exists
    for a given provider/CLI path, say so explicitly — a missing test is evidence.
 3. Which test helper builders default \`expected_epoch\` to None, and how many tests inherit it?
Return the structured list (use initial_class 'test-only' for pure test sites, but flag the
ones from question 1 in \`why\`).`,
  },
  {
    label: 'sweep:server-epoch-flow',
    prompt: `${PREAMBLE}

## Your sweep: the \`Option<ServerEpoch>\` dataflow out of the registry
Start at \`Registry::backend_server\` (src/registry/mod.rs:1587) and \`BackendServerRecord\`.
Enumerate EVERY caller (\`rg -n '\\.backend_server\\(' ${CRATE}/src\` — there are ~35) and for
each, classify how the \`Option\` is consumed:
  (a) \`.ok_or(...)\` / \`?\` / match that REFUSES on None  -> verified
  (b) passed straight into a struct field of type \`Option<...>\`  -> laundered
  (c) \`.unwrap_or(...)\` / \`if let Some\` that silently skips  -> laundered
  (d) compared for equality only when Some  -> laundered
Also find any OTHER registry accessor that can yield an epoch-shaped Option and its callers
(e.g. \`backend_instance_info\`, \`current_binding\`, \`NativeBinding::server_epoch\`,
\`registry/recovery.rs\`, \`registry/remote.rs\`). Report each consumption site.
Report file:line for each. This sweep is about the SOURCE side, so include sites that never
touch InventoryScope — an Option epoch laundered anywhere else is the same class.`,
  },
  {
    label: 'sweep:other-epoch-types',
    prompt: `${PREAMBLE}

## Your sweep: epoch-carrying types OTHER than InventoryScope
The review must not be scoped to one struct. Enumerate every type/function that carries or
verifies an expected epoch, and every site where verification is optional or skippable:
  - \`ManagedScope\` (src/ls_cli.rs ~739) and \`ScanTarget\` (src/ls_cli.rs ~752)
  - \`runtime::read_verified_ready_wez_descriptor\` / \`_in\` (src/runtime.rs:634,645,666) — the
    \`Option<Uuid>\` expected_epoch there has the same skip shape
  - \`gui.rs:3140\` \`expected_epoch: &str\`
  - \`gui_lifecycle.rs:547,563,568,832,1007\`
  - \`NativeBinding.server_epoch\` and \`binding_epoch\` (wez.rs:1184)
  - remote protocol: \`remote/protocol.rs\`, \`remote/agent.rs:2086,2092,2109\`, \`remote/routes.rs\`,
    \`remote/attach.rs\` — epoch fields on the wire and whether the agent enforces them
  - \`recovery.rs:1946,2080,2116\` snapshot epoch checks
  - \`operations.rs:1233,1348,1379,1404,2223,2766,640\`
  - \`GroupActivationResult\`/\`SplitDirectionResult\`/\`SplitResizeResult\`/\`SplitZoomResult\`
    \`server_epoch\` witnesses (backend/mod.rs) — is the witness ever actually compared, or
    returned and dropped?
Search for the SHAPE, not just these names: \`rg -n 'if let Some\\(expected\\)|is_some_and|unwrap_or\\(.*epoch|epoch.*Option<' ${CRATE}/src\`.
For each, say whether a None/absent epoch skips a check that a managed caller needed.`,
  },
  {
    label: 'sweep:provider-entrypoints',
    prompt: `${PREAMBLE}

## Your sweep: the provider surface — which entry points verify and which don't
Enumerate EVERY method on the Wez adapter (src/backend/wez.rs) and the tmux adapter
(src/backend/tmux.rs) that takes an \`&InventoryScope\`, plus every method on the
\`Provider\`/backend trait in src/backend/mod.rs. For each produce a row:
  method name | file:line | does it call a required-epoch helper (\`required_epoch\`,
  \`scope.expected_epoch.ok_or\`) or an optional one (\`if let Some(expected)\`) or neither |
  read or write.
This gives the definitive verified/unverified matrix at the provider boundary, which is where a
type-level fix would most plausibly land. Note especially:
  - wez.rs:1113 (scan sentinel epoch check), wez.rs:1184 (binding_epoch), wez.rs:1271 (required)
  - tmux.rs:466 (required), tmux.rs:478, tmux.rs:598 (read_markers), tmux.rs:1521
  - the \`verified_scan\` wrapper (wez.rs ~1160) and who passes \`expected: None\` to it
Report each method as a site (use \`enclosing\` for the method name and \`initial_class\` for
verified/unverified). In coverage_notes, state plainly: is there ANY read path on either
provider that verifies unconditionally?`,
  },
  {
    label: 'sweep:out-of-crate',
    prompt: `${PREAMBLE}

## Your sweep: the non-Rust surface that publishes and consumes the epoch
Find and read every shell/lua/plist/service file that participates in the epoch lifecycle:
  rg -l 'dmux' ${ROOT}/scripts ${ROOT}/config ${ROOT}/home 2>/dev/null | head -80
  specifically: dmux-mux-start.sh, any wezterm mux lua (sentinel workspace creation),
  \`_tmux-bootstrap\`, src/tmux_hook_cli.rs, src/bin/pane-bootstrap.rs, launchd/systemd units.
Establish, with file:line:
 1. The exact ORDER of: register backend instance row -> start server -> publish server_epoch.
    How wide is the window where the row exists with server_epoch NULL?
 2. Who writes \`server_epoch\` into the registry, via which dmux subcommand, and can it fail
    leaving NULL permanently?
 3. Does the tmux side have a sentinel equivalent to the Wez \`dmux:sentinel:<epoch>\` workspace,
    and how is a tmux server's epoch proven at read time (what does \`check_epoch\` actually
    query)?
 4. Any place OUTSIDE the crate that constructs an epoch-free scope or invokes a dmux read that
    would land on an unverified path.
Report as sites (file:line). Where a "site" is a shell line rather than a Rust construction,
still use the same record shape.`,
  },
]

const sweeps = (await parallel(SWEEPS.map((s) => () => agent(s.prompt, { label: s.label, phase: 'Enumerate', schema: SITE_SCHEMA, effort: 'high' })))).filter(Boolean)

// Barrier justified: dedup across six independent sweeps before per-site work.
const byKey = new Map()
for (const sweep of sweeps) {
  for (const s of sweep.sites || []) {
    const key = `${s.file}:${s.line}`
    const prev = byKey.get(key)
    if (!prev) {
      byKey.set(key, { ...s, seen_by: 1 })
    } else {
      prev.seen_by += 1
      if (prev.initial_class !== s.initial_class) {
        prev.why = `${prev.why} [DISAGREEMENT between sweeps: also classified ${s.initial_class} because: ${s.why}]`
      }
    }
  }
}
const allSites = Array.from(byKey.values())
log(`Enumerated ${allSites.length} distinct epoch-carrying sites across 6 sweeps`)

const coverageNotes = sweeps.map((s, i) => `### ${SWEEPS[i].label}\nmethod: ${s.method}\ngaps: ${s.coverage_notes}`).join('\n\n')

// Sites needing per-site adjudication: everything not obviously test-only, plus disagreements.
const needsWork = allSites.filter((s) => s.initial_class !== 'test-only' || /DISAGREEMENT|question 1|encode/i.test(s.why || ''))
log(`${needsWork.length} sites go to classify -> reachability`)

// ---------------------------------------------------------------------------
const SPECIALISTS = [
  {
    label: 'spec:tmux-side',
    prompt: `${PREAMBLE}

## Your task (review question 4): the tmux side, end to end
Most prior attention went to Wez. Establish, with file:line evidence, whether managed-tmux reads
are epoch-verified ANYWHERE.
 1. Read \`check_epoch\` in src/backend/tmux.rs in full. What does it actually query on the
    server, and what would a replacement tmux server on the same \`-L\` namespace return?
 2. tmux.rs:598 (\`read_markers\`) and tmux.rs:1521 — trace both. Who calls them with a managed
    instance, and with what epoch?
 3. Find the tmux \`inventory\`/scan implementation. Does it verify the epoch at all, or only
    \`if let Some\`? Compare to the Wez sentinel mechanism (wez.rs:1090-1120): Wez proves the
    epoch from a sentinel workspace present in the scan output. Does tmux have an equivalent
    proof, or does it ask the server to self-report? A server that self-reports its epoch is a
    WEAKER proof than a sentinel — if that is the case here, say so as a finding in its own
    right, because a hostile/replacement server can answer with any epoch.
 4. The managed-tmux bootstrap path: \`_tmux-bootstrap\`, src/tmux_hook_cli.rs. Where does a tmux
    server's epoch get published, and is the publish itself verified?
 5. Adjudicate the prior claim verbatim: "no managed-tmux read anywhere in the CLI is
    epoch-verified." TRUE, FALSE, or partially true — with the counterexample if false.
You may create a scratch tmux server for experiments ONLY with an isolated
\`TMUX_TMPDIR=${SCRATCH}/tmuxtmp\` and a unique \`-L\` socket name exported BEFORE the command
runs; never touch the user's default namespace, and kill only servers you started on your own
\`-L\` name.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:fence-states',
    prompt: `${PREAMBLE}

## Your task (review question 5): the fence and the register->publish->recover state machine
Claimed defect: src/ls_cli.rs ~779 — \`ScanTarget::Unpublished\` bypasses the operation fence
because \`ScanTarget::instance()\` returns \`None\` for it, so the instance is never added to
\`fenced\`. The register-then-publish window is precisely "unpublished AND possibly recovering",
yet the message tells the operator to restart the managed mux, which would kill an in-flight
recovery.
Do this:
 1. Read src/ls_cli.rs around 740-900 and 1130-1258 in full. Confirm or refute the claim about
    \`instance()\` and \`fenced\`. Quote the code.
 2. Map the actual state machine with file:line from: \`dmux-mux-start.sh\` (find it), 
    src/recovery.rs, src/registry/mod.rs (backend instance row lifecycle, \`server_epoch\`
    publish), src/registry/recovery.rs, src/operations.rs (the fence — find what \`fenced\`
    guards and what the lease/lock is).
 3. Enumerate the DISTINCT states a backend instance row can be in at read time:
      registered-not-yet-published / published-live / published-but-server-dead /
      recovering (in-flight restore) / mutating (fence held) / unaddressable / deregistered.
    For each: is it DISTINGUISHABLE at read time from the registry alone? With what column(s)?
 4. For each distinguishable state, what advice is SAFE to give the operator? Specifically:
    which states must NEVER produce "restart the mux" advice, and does current ls_cli emit that
    advice in any of them?
 5. Is the fence-bypass exploitable beyond bad advice — e.g. does skipping the fence let a
    concurrent recovery and a listing interleave in a way that corrupts output or state?
Report each as a finding with severity and reads_or_writes.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:audit-493e92c',
    prompt: `${PREAMBLE}

## Your task: adversarially audit the existing fix, commit 493e92c
Run \`git -C ${ROOT} show 493e92c\` and read the whole diff. Then:
 1. Verify the \`ScanTarget::Managed(_, ManagedScope)\` change actually closes the ls path. Is
    there any way to reach a scan from ls_cli with \`expected_epoch: None\` still? Check
    \`ScanTarget::Unregistered\` (ls_cli.rs:846) — is the tmux "discoverable" fallback reachable
    when a managed tmux instance EXISTS but with a NULL epoch, or when the registry row was
    deleted while the server lives? Trace \`backend_instance_for_backend\`. A managed server
    reachable through the Unregistered branch would be the same bug wearing a new name.
 2. Confirm/refute the claimed orphaned doc comment at ls_cli.rs ~1187 (\`scan_error_code\`).
 3. Audit the \`gui_lifecycle::validate_ready_descriptor\` change: epoch mismatch no longer
    retryable. Read the poll loop. Did making it terminal introduce a new failure mode — e.g.
    a legitimate republish during startup now hard-fails where it used to converge? Read
    src/runtime.rs:634-680 (\`read_verified_ready_wez_descriptor_in\`) — note its
    \`expected_epoch: Option<Uuid>\` has the SAME optional-skip shape at line 666; who passes
    None, and is that a managed caller?
 4. Did the fix introduce any NEW unverified path, dead code, or test that now asserts the old
    behaviour? Run \`cargo test -p dmux --no-run 2>&1 | tail -20\` to confirm it compiles.
 5. Is the \`ManagedScope\` type actually a barrier, or is it bypassable — can anyone still
    construct \`InventoryScope\` directly in ls_cli.rs? Is \`ManagedScope\` private to the module?
Report findings.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:false-leads',
    prompt: `${PREAMBLE}

## Your task: adjudicate the prior lists — find the item that is WRONG
The user states one item in the prior findings is already known to be wrong. Your job is to find
which, and to independently re-adjudicate BOTH lists rather than trusting either.

Reported UNFIXED (alleged laundering): adopt_cli.rs:238, rm_cli.rs:1138-1145, new_cli.rs:362-380,
gui_cli.rs:1430-1448, migrate_cli.rs:743, remote/agent.rs:1281, main.rs:1453,
space_cli.rs:1162, space_cli.rs:221-233.
Reported CLEAN: connect_cli.rs:1167, rm_cli.rs:1115, gui_lifecycle.rs:964, main.rs:1463,
gui_cli.rs:1392.

For EACH of the 14, read ~60 lines of surrounding context and decide independently:
 - Is the instance at that point genuinely managed (registry row + socket) or genuinely
   unmanaged (adoption-time discovery of a native the registry does not own)?
 - Does the value actually flow to a provider read, or is it dead / immediately overwritten /
   guarded by an earlier refusal?
 - Is a "clean" site clean only because the caller happens to hold Some today — i.e. is it one
   refactor away from laundering? That is a latent, not a defect; label it so.
Pay special attention to main.rs:1453 vs main.rs:1462 (adjacent, one reported dirty one clean)
and to space_cli.rs:221-233 where the epoch comes from a \`match socket\` — read what the two
match arms actually mean. Also read src/space_cli.rs:1771-1799: there is a test comment about a
"reconcile scope built with expected_epoch: None" making a branch unreachable — that is a prior
instance of this exact class; check whether it was actually fixed or only tested around.
Put every NOT-a-defect conclusion in \`refuted\` with the reason. Findings only for real defects.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:remote-path',
    prompt: `${PREAMBLE}

## Your task: the remote/agent write path
\`remote/agent.rs:1281\` builds a scope with a literal \`expected_epoch: None\` while
\`remote/agent.rs:1317\` (17 lines later) uses \`Some(epoch)\`. Read src/remote/agent.rs
1150-1450 in full and determine what the two branches are.
Then audit the whole remote surface for the class:
 - src/remote/agent.rs (2086-2130 epoch enforcement, 720, 884, 1398, 1417, 1581, 2392)
 - src/remote/routes.rs, src/remote/protocol.rs, src/remote/attach.rs (317, 617, 1640),
   src/remote/client.rs, src/remote/wez_compat.rs
Questions to answer with file:line:
 1. Can a REMOTE peer trigger an unverified scan on this host? Through which RPC method?
    Severity is higher for anything an enrolled peer can drive.
 2. Does the agent verify the epoch the CLIENT asserts against the registry, or does it trust
    the client's epoch? (agent.rs:2086-2110 looks like the enforcement point — is every mutating
    route funnelled through it, or only some?)
 3. Enumerate the remote routes and mark each verified/unverified. A route matrix is the
    deliverable. Check tests/remote_protocol/route_matrix.rs for the crate's own view and say
    whether it covers epoch verification or only routing.
 4. Does an unverified remote READ feed a subsequent WRITE (registry row, native mutation)?
Report findings ranked by whether a peer can reach them.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:dead-unreachable',
    prompt: `${PREAMBLE}

## Your task (review question 6): the "exists, is tested, is not reachable" pattern
This shape has appeared FIVE times in this codebase already:
  - \`inventory.rs\` (and the \`output\` renderers) had no production caller
  - \`output::confirmation_required\` was dead
  - \`registry::reconcile\` had no caller
  - a tmux scope built with \`expected_epoch: None\` made a whole branch unreachable
  - and now this epoch class
Hunt for FURTHER instances of the PATTERN, not just of this bug.
Detector recipe (run it, report the output):
 1. For every \`pub fn\` / \`pub(crate) fn\` in ${CRATE}/src, count non-test callers:
    build a list of public symbols, then for each grep the crate excluding \`#[cfg(test)]\`
    blocks and \`tests/\`. Symbols whose ONLY callers are tests are candidates.
    A workable approach: \`cargo build -p dmux 2>&1 | rg 'never used|never read|never
    constructed'\` first (dead_code warnings), then the manual sweep for the ones that are
    reachable-in-principle but have no production caller (which dead_code will NOT catch,
    because tests count as callers).
 2. Also hunt the sibling shapes:
    - enum variants that are constructed only in tests
    - match arms / \`if\` branches that no input can select (e.g. a condition on a field that is
      always None/always Some at every construction site) — the tmux-scope example above
    - trait methods with exactly one impl that always returns the same value
    - error variants never produced
    - config/CLI flags parsed but never read
 3. For each candidate: state whether it is (a) genuinely dead, (b) live but only via tests
    (the dangerous kind — it makes the suite green about something production never runs), or
    (c) reachable and fine. Ground each in file:line and name the test that covers it.
Prioritise anything on an epoch/verification/recovery path. Report each as a finding with
severity reflecting whether a GREEN TEST is currently vouching for unreachable code.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:conformance',
    prompt: `${PREAMBLE}

## Your task: what the governing spec actually REQUIRES
Read ${ROOT}/docs/dmux-wezterm-first-plan.md §2.3, §2.7, §8.2, §11, §14, §20.2 (acceptance case
25 especially), §21, and ${ROOT}/docs/adr/dmux/001-strict-endpoint-selection.md and
002-service-sentinel-startup.md in full.
Produce:
 1. The verbatim text of acceptance case 25 and any other case that bears on epoch verification,
    with the doc line numbers.
 2. A gap analysis: for each normative requirement about proving server identity, is it
    implemented, partially implemented, or absent — with the implementing file:line or a plain
    "absent".
 3. Specifically: does the spec REQUIRE that a managed read refuse without an epoch, or is it
    silent (making the current behaviour a spec gap rather than a spec violation)? The
    distinction matters for how the fix is framed. Quote the text either way.
 4. Does ADR 002 say anything about the register->publish window and what a reader should do in
    it? Does ADR 001 ("strict endpoint selection") already forbid probing an endpoint you cannot
    verify?
 5. Is there an acceptance test in tests/ that CLAIMS to cover case 25? Find it, read it, and
    say whether it actually exercises the wrong-server rejection or only the happy path.
Report gaps as findings.`,
    schema: REPORT_SCHEMA,
  },
  {
    label: 'spec:baseline',
    prompt: `${PREAMBLE}

## Your task: establish the objective baseline
 1. Run: \`cd ${ROOT}/scripts/rust && cargo test -p dmux -- --test-threads=1 2>&1 | tail -60\`
    This takes a while; let it finish. Expected baseline: 984 passed, 0 failed, 1 ignored.
    Report the ACTUAL numbers per test binary and in total. If it differs from 984/0/1, that is
    itself a finding — say which tests changed.
 2. Report the name of the 1 ignored test and why it is ignored.
 3. Run \`cd ${ROOT}/scripts/rust && cargo clippy -p dmux --all-targets 2>&1 | tail -40\` and
    report any warning that touches epoch/verification/dead code.
 4. Report \`git -C ${ROOT} status --porcelain\` before and after your run to PROVE you mutated
    nothing (the suite must not dirty the tree; if it does, that is a finding).
Do not fix anything. Return the numbers.`,
    schema: REPORT_SCHEMA,
  },
]

// Overlap: per-site adjudication pipeline runs concurrently with the specialists.
const [siteResults, specialistResults] = await parallel([
  () =>
    pipeline(
      needsWork,
      (site) =>
        agent(
          `${PREAMBLE}

## Your task: adjudicate ONE site
Site: ${site.file}:${site.line} (in \`${site.enclosing}\`)
Snippet as enumerated:
\`\`\`rust
${site.snippet}
\`\`\`
Enumerator said the epoch comes from: ${site.epoch_source}
Enumerator's initial class: ${site.initial_class} — because: ${site.why}

Re-derive this INDEPENDENTLY. Read at least 80 lines of surrounding context in the real file
(line numbers may have shifted; find the code, don't trust the line). Decide:
 - Is the instance MANAGED at this point (a registry backend_instances row this dmux owns) or
   genuinely UNMANAGED (native discovery / adoption of something the registry does not own)?
   Justify with the specific call that established it.
 - Does the scope flow into a provider READ, a provider WRITE, or neither?
 - Is verification actually performed on the path this scope takes, possibly elsewhere (an
   earlier \`required_epoch\`, a guard, a refusal)? Verification somewhere upstream makes the
   site safe — say where, with file:line.
Be willing to say 'verified' or 'intentionally-unmanaged'; over-reporting is as bad as
under-reporting. Return the structured verdict.`,
          { label: `classify:${site.file.split('/').pop()}:${site.line}`, phase: 'Classify', schema: CLASSIFY_SCHEMA },
        ),
      (verdict, site) => {
        if (!verdict || (verdict.verdict !== 'unverified-defect' && verdict.verdict !== 'unclear')) {
          return { site, verdict, reach: null }
        }
        return agent(
          `${PREAMBLE}

## Your task: PROVE REACHABILITY for one site (do not re-argue the classification)
Site: ${site.file}:${site.line} (\`${site.enclosing}\`)
Classifier's verdict: ${verdict.verdict} — ${verdict.rationale}
Classifier's evidence: ${verdict.evidence}
Read/write: ${verdict.reads_or_writes}

Presence is not reachability. Establish whether a REAL invocation can reach this site with
\`server_epoch = NULL\` or with a STALE epoch, and what the observable consequence is.
Required work:
 1. Build the call chain UPWARD from this site to a CLI subcommand entry point (src/main.rs
    dispatch) or an RPC route (src/remote/routes.rs). Give it as file:line -> file:line -> ...
    If you cannot reach an entry point, the site is unreachable — say so, that is a valid and
    valuable answer.
 2. State the exact command line a user/peer would type, and the registry/server state required
    (which row, which column NULL or stale).
 3. State the observable consequence, and be precise about severity:
      - a READ that misreports  -> medium/high
      - a WRITE to the registry on the basis of an unverified read -> high
      - a WRITE to a native server or pane markers (rename-workspace, set-option, kill) on the
        basis of an unverified read -> critical
    Name the specific write verb and its file:line.
 4. Say whether the epoch could be STALE rather than NULL, which is the more common real-world
    case (server restarted, registry not yet republished) — a stale epoch is caught by the
    \`Some\` branch, so a site is only defective via stale-epoch if it drops the Option entirely.
Set \`confidence\` honestly: 'proven-by-call-chain' requires you to have READ every hop.
Do NOT run the dmux binary in this task; a later phase does execution repros.`,
          { label: `reach:${site.file.split('/').pop()}:${site.line}`, phase: 'Reach', schema: REACH_SCHEMA },
        ).then((reach) => ({ site, verdict, reach }))
      },
    ),
  () => parallel(SPECIALISTS.map((s) => () => agent(s.prompt, { label: s.label, phase: 'Specialists', schema: s.schema, effort: 'high' }))),
])

const adjudicated = (siteResults || []).filter(Boolean)
const specialists = (specialistResults || []).filter(Boolean)
const defects = adjudicated.filter((r) => r.verdict && (r.verdict.verdict === 'unverified-defect' || r.verdict.verdict === 'unclear'))
const writers = defects.filter((r) => r.reach && r.reach.reachable !== 'no' && /writes/.test(r.reach.write_targets || r.verdict.reads_or_writes || ''))
log(`Adjudicated ${adjudicated.length} sites: ${defects.length} defects/unclear, ${writers.length} of them on write paths`)

// ---------------------------------------------------------------------------
phase('Repro')

const REPRO_HARNESS = `
## Reproduction rules — an earlier agent contaminated its own repro; do not repeat that
- Export EVERY env var BEFORE the binary runs, in the same command, e.g.:
    env XDG_DATA_HOME=${SCRATCH}/<yourname>/data \\
        DMUX_RUNTIME_DIR=${SCRATCH}/<yourname>/run \\
        TMUX_TMPDIR=${SCRATCH}/<yourname>/tmuxtmp \\
        ${ROOT}/scripts/rust/target/debug/dmux ...
  Never \`export\` in one Bash call and run the binary in another — shell state does not persist
  between Bash tool calls in this harness, so the second call would hit PRODUCTION paths.
- Work on a COPY of the registry:
    mkdir -p ${SCRATCH}/<yourname>/data/dmux
    cp $HOME/.local/share/dmux/registry.sqlite3 ${SCRATCH}/<yourname>/data/dmux/registry.sqlite3
  Confirm the binary is reading YOUR copy (check with \`dmux doctor\` or by mutating the copy and
  seeing the change) before drawing any conclusion.
- Verify isolation FIRST: run your command once against the pristine copy and confirm it does
  NOT touch $HOME/.local/share/dmux (compare \`stat\`/sha256 of the live file before and after).
  Print the proof. If the live file changed, ABORT and report that as a critical finding.
- A "stub server" answering on the socket is the way to prove wrong-server acceptance: a small
  script that speaks just enough of the protocol to answer a list. Put it in your scratch dir.
- NEVER stop/restart/kickstart a real mux server. NEVER set DMUX_WEZ_FIRST. NEVER touch the
  user's default tmux namespace — use your own \`-L\` name under your own TMUX_TMPDIR, and kill
  only what you started.
- Report the EXACT commands and the EXACT output. If the repro fails to reproduce, say so —
  that is a real result and it refutes the finding.
`

const reproTargets = writers.slice(0, 6)
if (reproTargets.length === 0) log('No write-path defects survived classification; running the two canonical repros anyway')

const REPROS = [
  {
    label: 'repro:ls-null-epoch',
    prompt: `${PREAMBLE}
${REPRO_HARNESS}

## Your repro (name yourself \`r1\`): does \`dmux ls\` accept an unverified server?
Claim to test: against a copy of the real registry with \`backend_instances.server_epoch\` NULLed
and a stub server answering as a DIFFERENT epoch, \`dmux ls --format json\` returned
\`{"ok":true,"errors":[]}\`, exit 0, demoted a live Space to \`"observation":"absent"\`, and
published the stranger's workspace as unmanaged.
This was allegedly FIXED by 493e92c (\`ScanTarget::Unpublished\` now refuses). So your job is to
determine the CURRENT behaviour at HEAD:
 1. Reproduce the setup: copy registry, \`sqlite3 ... "UPDATE backend_instances SET server_epoch
    = NULL WHERE ..."\` on the COPY, point socket_path at a stub socket you control.
 2. Run \`dmux ls --format json\` with the scratch env. Record exit code and full JSON.
 3. Report whether it now REFUSES (expected post-fix) and with what error code and message.
 4. Then test the OTHER half, which the fix may not cover: leave \`server_epoch\` SET but make the
    stub answer with a DIFFERENT epoch. Does ls reject it? What error code?
 5. And the third case: DELETE the backend_instances row entirely while a server still answers
    on the discoverable tmux namespace — does ls fall through to \`ScanTarget::Unregistered\`
    (ls_cli.rs:846, \`expected_epoch: None\`) and scan the server unverified? This is the
    suspected surviving hole. Prove or refute it.
Report each as a finding with the verbatim command and output.`,
  },
  {
    label: 'repro:adopt-mutation',
    prompt: `${PREAMBLE}
${REPRO_HARNESS}

## Your repro (name yourself \`r2\`): does \`dmux adopt\` MUTATE an unverified server?
Claim to test: \`dmux adopt native:wez:<token>\` scanned an unverified server and then mutated
it — issuing \`rename-workspace --window-id 1 --if-workspace dmux:ws:someone-elses
dmux:<host>:<space_uid>\` — and wrote a registry row, advancing \`authority_revision\` 36->38.
It landed \`aborted\` only because the stub's \`list\` output was static.
Your job:
 1. Determine the CURRENT behaviour at HEAD (adopt_cli.rs:238 is reported still unfixed).
 2. Set up as in the harness. Use a stub \`wezterm\` binary (dmux invokes \`wezterm cli ...\`;
    find how the binary path is resolved — see \`crate::runtime::production_wez_paths\` and the
    \`with_wez\` seam — and point it at your stub via whatever env/config seam exists WITHOUT
    touching production config). Log every argv the stub receives to a file.
 3. Run the adopt against the scratch registry. Capture: exit code, JSON, the argv log (does a
    \`rename-workspace\` appear?), and the registry \`authority_revision\` before/after
    (\`sqlite3\` the copy).
 4. Make the stub's list output DYNAMIC (respond post-rename as if the rename succeeded) to test
    the claim that "against a real replacement server the rename succeeds". Does the adopt then
    complete and write a binding row?
 5. Also try \`dmux migrate\` — claim is it returned ok:true and planned
    \`"disposition":"adopt"\` for the stranger's workspace.
Report the verbatim evidence. If you cannot wire the stub, say EXACTLY what blocked you rather
than inferring the outcome.`,
  },
  {
    label: 'repro:tmux-managed-read',
    prompt: `${PREAMBLE}
${REPRO_HARNESS}

## Your repro (name yourself \`r3\`): the managed-tmux read path
Using a REAL tmux server you start yourself under \`TMUX_TMPDIR=${SCRATCH}/r3/tmuxtmp\` and a
unique \`-L\` name (never the user's), plus a scratch registry copy:
 1. Register/point a managed tmux instance in the scratch registry at YOUR \`-L\` socket. (Read
    the schema: \`sqlite3 <copy> '.schema backend_instances'\`. You may need to insert or UPDATE
    a row on the copy — that is fine, it is a copy.)
 2. With \`server_epoch\` set to a UUID that does NOT match your server, run the managed-tmux
    reads: \`dmux ls\`, \`dmux space ...\` (whichever subcommands reach tmux reads — the
    reachability phase will have identified them; if unsure, try ls and space list).
    Does dmux detect the wrong server? What does tmux's \`check_epoch\` actually compare?
 3. With \`server_epoch\` NULL, same commands. Does it refuse or scan?
 4. Kill YOUR tmux server, start a NEW one on the SAME \`-L\` name (this simulates the
    socket-stealing replacement, safely, in your own namespace) with a different set of
    sessions. Re-run the reads with the OLD epoch still in the registry. Is the replacement
    detected? THIS IS THE CORE OF ACCEPTANCE CASE 25 — get a definitive answer.
 5. Clean up: kill only servers on your own \`-L\` name; \`tmux -L <yours> kill-server\`.
Report verbatim commands and output. State explicitly what remains untested.`,
  },
  ...reproTargets.map((t, i) => ({
    label: `repro:site-${i + 1}`,
    prompt: `${PREAMBLE}
${REPRO_HARNESS}

## Your repro (name yourself \`s${i + 1}\`): a write-path defect found by this review
Site: ${t.site.file}:${t.site.line} (\`${t.site.enclosing}\`)
Claimed reachability: ${t.reach.invocation}
Preconditions: ${t.reach.preconditions}
Claimed consequence: ${t.reach.consequence}
Write targets: ${t.reach.write_targets}
Call chain: ${t.reach.evidence}

Attempt to reproduce this EXECUTABLY against a scratch registry copy and stub/scratch servers.
If a full repro is impossible without a live canary, get as close as you can (e.g. a targeted
\`#[test]\` you write in ${SCRATCH} as a standalone harness, or driving the library API through
\`cargo test\` — but DO NOT add tests to the crate's tests/ directory or edit crate source).
State plainly: reproduced / partially reproduced / not reproduced, and what is still untested.`,
  })),
]

const FIXSHAPE = [
  {
    label: 'fix:registry-accessor',
    approach: 'Make the registry accessor the boundary: split `backend_server` into a fallible `published_server_epoch(instance) -> Result<ServerEpoch>` that errors on NULL, plus an explicit `server_epoch_opt` for the two or three callers that genuinely need the Option (recovery, publish).',
  },
  {
    label: 'fix:scope-constructor',
    approach: 'Make `InventoryScope` construction the boundary: private fields + two constructors, `InventoryScope::managed(backend, endpoint, ServerEpoch)` (non-Option) and `InventoryScope::unmanaged(backend, endpoint)`, so a managed scope cannot be built without an epoch. Possibly promote `ls_cli::ManagedScope` into `backend/mod.rs` as the crate-wide type.',
  },
  {
    label: 'fix:provider-entry',
    approach: 'Make the provider entry the boundary: split the read surface into `inventory_verified(&VerifiedScope)` and `inventory_discover(&DiscoveryScope)` as distinct methods with distinct types, so the `if let Some(expected)` skip disappears from the adapters entirely and the type of the scope decides which is callable.',
  },
  {
    label: 'fix:sum-type',
    approach: 'Replace `expected_epoch: Option<ServerEpoch>` with an explicit sum type `EpochExpectation::{Pinned(ServerEpoch), Unmanaged}` (no `None`), so every match is exhaustive, every skip is a deliberate written-out `Unmanaged` arm, and grep finds all of them. Cheapest to land; weakest guarantee.',
  },
]

const [reproResults, fixProposals] = await parallel([
  () => parallel(REPROS.map((r) => () => agent(r.prompt, { label: r.label, phase: 'Repro', effort: 'high', isolation: 'worktree' }))),
  () =>
    parallel(
      FIXSHAPE.map((f) => () =>
        agent(
          `${PREAMBLE}

## Your task (review question 3): design ONE fix shape and argue it honestly
Nine-plus site-by-site patches is HOW THIS CLASS SURVIVED IN THE FIRST PLACE. The question is
whether the type system should make the unverified case UNREPRESENTABLE for managed instances,
and where that boundary belongs.

YOUR assigned approach:
${f.approach}

Deliverable (as prose, be concrete):
 1. The exact type/signature changes, with the file:line of each thing you would change.
 2. A blast-radius count: how many call sites must change, and name the awkward ones. Actually
    grep and count — do not estimate.
 3. Which of the enumerated defect sites this shape closes AUTOMATICALLY (the compiler forces
    it) versus which still need a judgement call at the call site. The value of a type-level fix
    is exactly the size of the first set.
 4. The honest downsides: what does it make harder? Which legitimate unmanaged callers get more
    verbose? Does it force a fallible API where none existed? Does it churn the test suite (984
    tests) and how much?
 5. Migration order: can it land incrementally without a flag day? What lands first?
 6. What it does NOT fix — the residual risk after this change. Be specific; every one of these
    approaches leaves something open.
Do NOT edit any crate source. This is a design deliverable. Argue the tradeoff, do not assert.`,
          { label: f.label, phase: 'FixShape', effort: 'high' },
        ),
      ),
    ),
])

const repros = (reproResults || []).filter(Boolean)
const proposals = (fixProposals || []).filter(Boolean)
log(`Repros: ${repros.length}; fix proposals: ${proposals.length}`)

// ---------------------------------------------------------------------------
phase('Verify')

// Pool every candidate finding from specialists + reachability into one list.
const candidates = []
for (let i = 0; i < specialists.length; i++) {
  for (const f of specialists[i].findings || []) {
    candidates.push({
      source: SPECIALISTS[i].label,
      title: f.title,
      site: f.site,
      severity: f.severity,
      rw: f.reads_or_writes,
      reach: f.reachability,
      evidence: f.evidence,
    })
  }
}
for (const d of defects) {
  if (!d.reach || d.reach.reachable === 'no') continue
  candidates.push({
    source: 'site-adjudication',
    title: `Unverified epoch at ${d.site.file}:${d.site.line} (${d.site.enclosing})`,
    site: `${d.site.file}:${d.site.line}`,
    severity: d.reach.severity,
    rw: d.reach.write_targets,
    reach: `${d.reach.invocation} | preconditions: ${d.reach.preconditions} | consequence: ${d.reach.consequence}`,
    evidence: `${d.verdict.evidence} || chain: ${d.reach.evidence} || confidence: ${d.reach.confidence}`,
  })
}

// --- Dedup. Agents cite the same defect with absolute vs repo-relative paths, with the
// construction line vs the laundering line a few lines above, and from several sweeps at once.
// Without this the fan-out multiplies duplicates by the lens count.
const normSite = (raw) => {
  const s = String(raw || '')
  const m = s.match(/([A-Za-z0-9_.\/-]+\.(?:rs|zsh|lua|sh|toml|json|md))\s*:\s*(\d+)/)
  if (!m) return { file: `unparsed:${s.slice(0, 48)}`, line: 0 }
  let file = m[1]
  file = file.replace(/^\/Users\/fredrir\/dotfiles\//, '')
  file = file.replace(/^.*?scripts\/rust\/crates\/dmux\//, '')
  file = file.replace(/^\.\//, '')
  if (!file.startsWith('src/') && !file.includes('/') && file.endsWith('.rs')) file = `src/${file}`
  return { file, line: parseInt(m[2], 10) }
}

const SEV_RANK = { critical: 0, high: 1, medium: 2, low: 3, info: 4 }
const RW_RANK = {
  'writes-native-server': 0,
  'writes-multiple': 1,
  'writes-pane-markers': 2,
  'writes-registry': 3,
  read: 4,
  none: 5,
}
const bestBy = (vals, rank, dflt) =>
  vals.filter((v) => v in rank).sort((a, b) => rank[a] - rank[b])[0] || dflt

const byFile = new Map()
for (const c of candidates) {
  const n = normSite(c.site)
  if (!byFile.has(n.file)) byFile.set(n.file, [])
  byFile.get(n.file).push({ ...c, _line: n.line })
}
const merged = []
for (const [file, list] of byFile) {
  list.sort((a, b) => a._line - b._line)
  let cur = null
  for (const c of list) {
    // Same file within 20 lines of the group anchor == one construction site / one defect.
    if (cur && c._line - cur._anchor <= 20) {
      cur._members.push(c)
      if (!cur._lines.includes(c._line)) cur._lines.push(c._line)
    } else {
      cur = { _file: file, _anchor: c._line, _lines: [c._line], _members: [c] }
      merged.push(cur)
    }
  }
}

const pooled = merged.map((g) => {
  const m = g._members
  const sev = bestBy(m.map((x) => x.severity), SEV_RANK, 'medium')
  const rw = bestBy(m.map((x) => x.rw), RW_RANK, 'read')
  const sources = Array.from(new Set(m.map((x) => x.source)))
  return {
    site: `${g._file}:${g._lines.join('/')}`,
    severity: sev,
    rw,
    source: sources.join(', '),
    dup_count: m.length,
    title: m[0].title,
    all_titles: Array.from(new Set(m.map((x) => x.title))).join(' ;; '),
    reach: Array.from(new Set(m.map((x) => `[${x.source}] ${x.reach}`))).join('\n'),
    evidence: Array.from(new Set(m.map((x) => `[${x.source}] ${x.evidence}`))).join('\n'),
  }
})
pooled.sort((a, b) => (SEV_RANK[a.severity] - SEV_RANK[b.severity]) || (RW_RANK[a.rw] - RW_RANK[b.rw]))

log(`${candidates.length} raw candidates -> ${pooled.length} distinct sites after dedup`)

// --- Tier the verification effort. Three independent lens agents where a wrong answer is
// expensive (critical/high); one combined-lens agent for medium/low; info passes through
// unverified and clearly labelled. This is what keeps the phase ~75 agents instead of ~300.
const HIGH = pooled.filter((c) => c.severity === 'critical' || c.severity === 'high')
const MID = pooled.filter((c) => c.severity === 'medium' || c.severity === 'low')
const INFO = pooled.filter((c) => c.severity === 'info')

// No candidate is dropped — capping coverage is the silent-truncation failure mode this very
// review exists to find. Instead, scale the LENS DEPTH on critical/high to hit an agent budget,
// so every finding still gets at least one adversarial verifier.
const TARGET_AGENTS = 90
let highLenses = 3
if (HIGH.length * 3 + MID.length > TARGET_AGENTS) highLenses = 2
if (HIGH.length * 2 + MID.length > TARGET_AGENTS) highLenses = 1
const highRun = HIGH
const midRun = MID
const dropped = []
log(`Verify budget: ${HIGH.length} critical/high x${highLenses} ${highLenses === 3 ? 'independent lenses' : highLenses === 2 ? 'lenses (severity lens folded into correctness)' : 'combined-lens pass (high load)'} + ${MID.length} medium/low x1 combined = ${HIGH.length * highLenses + MID.length} agents; ${INFO.length} info passed through unverified; 0 dropped`)

const LENSES = [
  { key: 'reachability', ask: 'Attack the REACHABILITY claim. Walk the call chain yourself from the claimed entry point. Is there an earlier guard, refusal, or required-epoch call that makes this unreachable? Is the entry point actually wired in main.rs dispatch? Does the precondition state actually occur in practice, or only if someone hand-edits the DB?' },
  { key: 'correctness', ask: 'Attack the CODE-READING. Read the cited file:line yourself. Did the finder misread the control flow, confuse a managed with an unmanaged instance, cite a line that moved, or describe behaviour that a different branch handles? Are the line numbers accurate at HEAD?' },
  { key: 'severity', ask: 'Attack the SEVERITY and the write claim. Does this path actually WRITE anything, or was a write inferred? Is a claimed native mutation actually guarded by the mutation-side required_epoch (wez.rs:1271 / tmux.rs:466), which would downgrade it to a read-misreport? Refute if the severity is inflated even when the finding is real — say so in `correction`.' },
]

const COMBINED_ASK = `Attack this finding on ALL THREE fronts, in order, and say what you found on each:
 (a) REACHABILITY — walk the call chain yourself from the claimed entry point. Is there an
     earlier guard, refusal, or required-epoch call that makes it unreachable? Is the entry
     point actually wired in main.rs dispatch? Does the precondition state occur in practice,
     or only if someone hand-edits the DB?
 (b) CODE-READING — read the cited file:line yourself. Did the finder misread control flow,
     confuse a managed with an unmanaged instance, cite a line that has moved, or describe
     behaviour a different branch handles?
 (c) SEVERITY — does this path actually WRITE, or was the write inferred? Is a claimed native
     mutation already guarded by the mutation-side required_epoch (wez.rs:1271 / tmux.rs:466),
     which would downgrade it to a read-misreport?
Refute if ANY of the three collapses the finding. If the finding is real but misstated on one
front, set refuted:false and put the corrected claim in \`correction\`.`

const claimBlock = (c) => `CLAIM: ${c.title}
Site: ${c.site}${c.dup_count > 1 ? `   (independently reported ${c.dup_count}x — corroboration, not confirmation; the reporters may share a mistake)` : ''}
Severity claimed: ${c.severity}   Read/write claimed: ${c.rw}
${c.dup_count > 1 ? `All phrasings of this finding:\n${c.all_titles}\n` : ''}Reachability claimed:
${c.reach}
Evidence offered:
${c.evidence}
Reported by: ${c.source}`

const shortLabel = (c) => `${c.site.split('/').pop()}`

const verifiedHigh = pipeline(highRun, (c) =>
  parallel(
    LENSES.slice(0, highLenses).map((l) => () =>
      agent(
        `${PREAMBLE}

## Your task: REFUTE this finding through the ${l.key} lens
You are an adversarial verifier. Your default is REFUTED. Only let the finding stand if you
personally re-derived it from the source. Do not be agreeable.

${claimBlock(c)}

${highLenses === 1 ? COMBINED_ASK : l.ask}

Set \`refuted: true\` if the claim is wrong, unreachable, or unsupported by the code you read.
Set \`refuted: false\` ONLY if you verified it yourself and cite the file:line you read.
If the claim is real but MISSTATED (wrong severity, wrong mechanism, wrong line), set
refuted:false and put the corrected version in \`correction\`.`,
        { label: `verify:${l.key}:${shortLabel(c)}`, phase: 'Verify', schema: VERDICT_SCHEMA },
      ),
    ),
  ).then((votes) => {
    const v = (votes || []).filter(Boolean)
    const stands = v.filter((x) => !x.refuted).length
    return {
      ...c,
      votes: v.map((x, i) => `${LENSES[i] ? LENSES[i].key : 'lens'}: ${x.refuted ? 'REFUTED' : 'STANDS'} — ${x.reason}${x.correction ? ` | correction: ${x.correction}` : ''}`),
      survives: stands >= Math.ceil(v.length / 2),
      vote_count: `${stands}/${v.length} lenses stand`,
    }
  }),
)

const verifiedMid = pipeline(midRun, (c) =>
  agent(
    `${PREAMBLE}

## Your task: REFUTE this finding (combined three-lens pass)
You are an adversarial verifier. Your default is REFUTED. Only let the finding stand if you
personally re-derived it from the source. Do not be agreeable. You are the ONLY verifier this
finding gets, so do all three checks properly rather than skimming.

${claimBlock(c)}

${COMBINED_ASK}

Set \`refuted: true\` if the claim is wrong, unreachable, or unsupported by the code you read.
Set \`refuted: false\` ONLY if you verified it yourself and cite the file:line you read.
In \`reason\`, state your finding on each of (a), (b), (c) separately.`,
    { label: `verify:combined:${shortLabel(c)}`, phase: 'Verify', schema: VERDICT_SCHEMA },
  ).then((v) => ({
    ...c,
    votes: v ? [`combined(a,b,c): ${v.refuted ? 'REFUTED' : 'STANDS'} — ${v.reason}${v.correction ? ` | correction: ${v.correction}` : ''}`] : ['combined: NO VERDICT (agent failed)'],
    survives: v ? !v.refuted : false,
    vote_count: v ? '1/1 combined-lens stands' : 'no verdict',
  })),
)

const [vHigh, vMid] = await parallel([() => verifiedHigh, () => verifiedMid])
const verified = [
  ...(vHigh || []).filter(Boolean),
  ...(vMid || []).filter(Boolean),
  ...INFO.map((c) => ({ ...c, votes: ['not verified: info severity, passed through'], survives: false, vote_count: 'unverified (info)' })),
  ...dropped.map((c) => ({ ...c, votes: ['NOT VERIFIED: capped out of the verification budget'], survives: false, vote_count: 'unverified (capped)' })),
]

const confirmed = (verified || []).filter(Boolean).filter((c) => c.survives)
const killed = (verified || []).filter(Boolean).filter((c) => !c.survives && !/unverified/.test(c.vote_count))
const unverifiedPassthrough = (verified || []).filter(Boolean).filter((c) => /unverified/.test(c.vote_count))
log(`${confirmed.length} findings survived adversarial verification; ${killed.length} refuted; ${unverifiedPassthrough.length} passed through unverified (info/capped)`)

// ---------------------------------------------------------------------------
phase('Critic')

const critique = await agent(
  `${PREAMBLE}

## Your task: completeness critic — what did this review MISS?
A multi-agent review just ran. Here is what it covered:

ENUMERATION: ${allSites.length} sites across 6 sweeps.
Sweep coverage notes (read the gaps carefully):
${coverageNotes}

CONFIRMED FINDINGS (${confirmed.length}):
${confirmed.map((c) => `- [${c.severity}/${c.rw}] ${c.title} @ ${c.site} (${c.vote_count})`).join('\n') || '(none)'}

REFUTED (${killed.length}):
${killed.map((c) => `- ${c.title} @ ${c.site} — ${c.votes.join(' ;; ')}`).join('\n') || '(none)'}

NOT ADVERSARIALLY VERIFIED (${unverifiedPassthrough.length}) — info-severity or capped out of the
verification budget. These are NEITHER confirmed NOR refuted; treat them as open questions:
${unverifiedPassthrough.map((c) => `- [${c.severity}/${c.rw}] ${c.title} @ ${c.site}`).join('\n') || '(none)'}

SPECIALIST SUMMARIES:
${specialists.map((s, i) => `### ${SPECIALISTS[i].label}\n${s.summary}\nUNTESTED: ${(s.untested || []).join('; ')}\nREFUTED: ${(s.refuted || []).join('; ')}`).join('\n\n')}

REPRO RESULTS:
${repros.map((r, i) => `### ${REPROS[i].label}\n${String(r).slice(0, 3000)}`).join('\n\n')}

Now be the harshest possible critic. The user's explicit warning: "the prior list grew twice and
was incomplete both times; anchoring to a boundary is the main way this review fails."
Answer, doing your own searching — do not just reason over the summaries:
 1. What FILE or MODULE did nobody look at? Cross-check the enumeration against
    \`find ${CRATE}/src -name '*.rs'\` and name the untouched ones. Are any of them on an epoch
    path? (Candidates nobody may have opened: src/attach.rs, src/resolve.rs, src/policy.rs,
    src/doctor.rs, src/list.rs, src/inventory.rs, src/state.rs, src/hosts.rs, src/refs.rs,
    src/bootstrap.rs, src/keys.rs, src/history.rs, src/locks.rs, src/childio.rs,
    src/registry/reconcile.rs, src/registry/bootstrap_journal.rs, src/registry/hosts.rs,
    src/bin/pane-bootstrap.rs, src/tmux_hook_cli.rs.) GO OPEN THEM and grep each for epoch,
    scope, and provider reads.
 2. What MODALITY was not run? (e.g. nobody diffed against the last known-good review; nobody
    checked \`git log -S expected_epoch\` for when the Option was introduced and whether the
    original commit message admits it; nobody looked at the fuzz/property tests if any.)
    Run \`git -C ${ROOT} log --oneline -S 'expected_epoch' -- scripts/rust/crates/dmux | head -30\`
    and see whether the history reveals a deliberate decision or an accident.
 3. Which CONFIRMED finding is still only 'proven-by-call-chain' and never executed? Name them —
    those are the ones most likely to be wrong.
 4. Is there a finding that SHOULD exist by symmetry but nobody filed? E.g. if wez has a hole at
    X, does tmux have the mirror hole, and vice versa? If reads are unverified, are there other
    *proof* mechanisms (socket dev/ino in \`backend_server\`! — \`socket_dev\`/\`socket_ino\` are
    columns that exist; is that stat-based identity check ever actually enforced, or is it
    another 'exists, tested, unreachable' instance?) GO CHECK socket_dev/socket_ino usage.
 5. What did the user ask for that has no answer yet? Re-read the six numbered asks in the
    context above and name any that is under-served.
Return concrete NEW work items and any NEW findings you personally verified, with file:line.
Be specific and evidence-grounded; "consider looking at X" is not acceptable output.`,
  { label: 'critic:completeness', phase: 'Critic', effort: 'xhigh' },
)

// One more round on whatever the critic surfaced.
const followups = await agent(
  `${PREAMBLE}

## Your task: chase down the critic's gaps and close them
The completeness critic reported:
${String(critique).slice(0, 20000)}

For EVERY concrete gap and every new candidate finding above, do the verification work now:
open the files, run the greps, build the call chains. Confirm or refute each with file:line.
Pay particular attention to any 'exists, tested, unreachable' candidates and to the
socket_dev/socket_ino identity check question.
Return a structured report: confirmed new findings, refuted critic suggestions, and anything
still untested with the reason.`,
  { label: 'critic:followup', phase: 'Critic', schema: REPORT_SCHEMA, effort: 'high' },
)

// ---------------------------------------------------------------------------
phase('Synthesis')

const report = await agent(
  `${PREAMBLE}

## Your task: write the final review report
You are synthesising an independent adversarial review. Write the DELIVERABLE the user asked
for. Do not pad. Every claim must carry file:line. Verify any line number you quote by opening
the file — earlier agents may have cited shifted lines.

Write the report to ${SCRATCH}/EPOCH-REVIEW.md AND return it as your final text (the full text,
not a summary of it — your return value IS the report).

## Required structure
1. **Verdict** — 3-6 sentences. Is the class closed, partially closed, or open? What is the
   single most serious thing found?
2. **Ranked findings table** — columns: # | site (file:line) | severity | reads/writes |
   reachability (proven-by-execution / proven-by-call-chain / plausible) | evidence.
   Ranked by severity, with writes above reads at equal severity. Under each row, 2-4 sentences
   of detail: the mechanism, the invocation, the consequence.
3. **The complete site enumeration** — every InventoryScope-and-friends construction site,
   classified verified / unverified / unreachable / intentionally-unmanaged / test-only, as a
   compact table. State explicitly HOW managed-laundering was distinguished from
   genuinely-unmanaged. This is review-ask #1 and must be complete, not a sample.
4. **The tmux answer** (review-ask #4) — is any managed-tmux read epoch-verified anywhere?
   Adjudicate the prior reviewer's sentence verbatim.
5. **The fence / state-machine answer** (review-ask #5) — the distinguishable states table and
   which operator advice is safe in each.
6. **Recommended fix shape** (review-ask #3) — pick ONE, argue the tradeoff against the other
   three, give the blast radius, the migration order, and the residual risk. Argue; do not
   assert. Name where the boundary belongs and why THAT boundary and not the others.
7. **'Exists, tested, unreachable' — further instances** (review-ask #6) — the sixth+ instances
   of the pattern, with the test that is currently vouching for each.
8. **Refuted / false leads** — every reported site that is NOT a defect, with the reason.
   Explicitly identify which prior-list item was the known-wrong one, or say you could not
   identify a single wrong one and which is the best candidate.
9. **What remains untested and why** — exact, honest, itemised. Anything needing a live canary,
   a reboot, a real replacement server, or a state you refused to create because it would
   mutate live state. This section is mandatory and must not be hedged.
10. **Suggested next actions** — ordered, concrete.

## Inputs

### SITE ENUMERATION (${allSites.length} sites)
${JSON.stringify(allSites).slice(0, 90000)}

### SWEEP COVERAGE GAPS
${coverageNotes}

### PER-SITE ADJUDICATION
${JSON.stringify(adjudicated.map((a) => ({ site: `${a.site.file}:${a.site.line}`, fn: a.site.enclosing, verdict: a.verdict, reach: a.reach }))).slice(0, 90000)}

### SPECIALIST REPORTS
${specialists.map((s, i) => `#### ${SPECIALISTS[i].label}\n${JSON.stringify(s).slice(0, 22000)}`).join('\n\n')}

### REPRODUCTION RESULTS (execution evidence — weight these above code-reading)
${repros.map((r, i) => `#### ${REPROS[i].label}\n${String(r).slice(0, 14000)}`).join('\n\n')}

### FIX-SHAPE PROPOSALS
${proposals.map((p, i) => `#### ${FIXSHAPE[i].label}\n${String(p).slice(0, 14000)}`).join('\n\n')}

### ADVERSARIALLY CONFIRMED FINDINGS (${confirmed.length})
${JSON.stringify(confirmed).slice(0, 60000)}

### REFUTED FINDINGS (${killed.length}) — report these in section 8, do not resurrect them
${JSON.stringify(killed).slice(0, 40000)}

### NOT ADVERSARIALLY VERIFIED (${unverifiedPassthrough.length}) — info-severity or capped out of
the verification budget. Neither confirmed nor refuted. They MUST appear in section 9 ("what
remains untested") with the reason, never in the ranked findings table as if they were verified.
${JSON.stringify(unverifiedPassthrough).slice(0, 20000)}

### COMPLETENESS CRITIC
${String(critique).slice(0, 20000)}

### CRITIC FOLLOW-UP (verified)
${JSON.stringify(followups).slice(0, 30000)}

Where sources conflict, side with EXECUTION evidence over call-chain reasoning, and with the
adversarial verifiers over the original finder. Where a finding survived 2/3 rather than 3/3,
say so in the table. Do not report a finding the verifiers killed as if it were live.`,
  { label: 'synthesis:report', phase: 'Synthesis', effort: 'xhigh' },
)

return {
  report,
  counts: {
    sites_enumerated: allSites.length,
    sites_adjudicated: adjudicated.length,
    defects: defects.length,
    candidates: candidates.length,
    confirmed: confirmed.length,
    refuted: killed.length,
    repros: repros.length,
  },
  report_path: `${SCRATCH}/EPOCH-REVIEW.md`,
}
