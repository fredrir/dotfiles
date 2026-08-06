# Review rubric

## Contents
- How to use this
- A. Validity (blocking)
- B. Triggering
- C. Scope and value
- D. Context economy
- E. Instruction quality
- F. Structure and navigation
- G. Scripts and tools
- H. Safety audit
- I. Maintainability
- Scoring and verdict
- Reviewing someone else's skill

## How to use this

Walk the sections in order. A fails in section A block everything else — fix them first. For
sections B-I, mark each item pass / fail / not applicable, and write one line of evidence for
every fail, quoting the offending text. Vague findings ("description could be better") don't
produce fixes; "description names no file types and no trigger phrases" does.

Report findings grouped by severity, not by section order, so the user sees what matters first.

## A. Validity (blocking)

- [ ] File is named exactly `SKILL.md`, case-sensitive
- [ ] YAML frontmatter opens and closes with `---` and parses
- [ ] `name` present: 1-64 chars, lowercase letters/digits/hyphens only
- [ ] `name` has no leading, trailing, or consecutive hyphens
- [ ] `name` does not contain the reserved words "anthropic" or "claude"
- [ ] `name` matches the containing directory name
- [ ] `description` present, non-empty, ≤1024 characters
- [ ] No XML angle brackets anywhere in frontmatter
- [ ] Frontmatter fields limited to `name`, `description`, `license`, `compatibility`,
      `metadata`, `allowed-tools` — or the target client is confirmed to accept the extras
- [ ] All file paths use forward slashes
- [ ] Every referenced file actually exists at the stated relative path

## B. Triggering

- [ ] Description states **what** the skill does
- [ ] Description states **when** to use it, with concrete contexts
- [ ] Written in third person (not "I can..." or "You can...")
- [ ] Contains the vocabulary a user would actually type, including file types and tool names
- [ ] Covers at least one case where the user wouldn't name the domain explicitly
- [ ] Has a boundary clause if an adjacent skill could be confused with it
- [ ] All "when to use" information lives in the description, not buried in the body
- [ ] Under 1024 chars — descriptions grow during tuning and silently break

Two failure directions, and they need opposite fixes: too narrow means it won't fire on rephrased
requests; too broad means it fires on near-misses. Section B alone can't tell you which — that
needs the trigger test in `evaluation.md`.

## C. Scope and value

- [ ] The task genuinely fails or degrades without the skill (baseline was actually run)
- [ ] Scope is one coherent unit of work, not a grab bag
- [ ] Content reflects real expertise — specific conventions, schemas, gotchas — rather than
      generic advice an agent already knows
- [ ] Doesn't duplicate or contradict another installed skill
- [ ] Teaches a generalizable method, not the answer to one specific instance

The sharpest test for section C: find the single most valuable sentence in the skill. If it's
something like "follow best practices" or "handle errors appropriately", the skill has no content.

## D. Context economy

- [ ] `SKILL.md` body is under 500 lines
- [ ] Roughly under 5k tokens, or the excess is justified
- [ ] No explanations of things any competent agent already knows
- [ ] Detailed reference material lives in separate files, not inline
- [ ] Nothing in the body is there "for completeness"

Test each paragraph: *would the agent get this wrong without it?* If no, cut. If unsure, cut it and
see whether the evals move.

## E. Instruction quality

- [ ] Specificity matches fragility — prescriptive where operations are fragile, directional where
      several approaches work
- [ ] Reasoning is given for non-obvious instructions ("do X because Y")
- [ ] Not padded with capitalized ALWAYS/NEVER in place of explanation
- [ ] One default is given per decision, not a menu of equivalent options
- [ ] Terminology is consistent — one word per concept throughout
- [ ] No time-sensitive content (or it's quarantined in an "Old patterns" section)
- [ ] Examples are concrete, with real input/output pairs where format matters
- [ ] A gotchas section exists if the domain has any non-obvious traps
- [ ] Multi-step workflows have explicit ordered steps
- [ ] Quality-critical steps have a validation loop (do → check → fix → recheck)

## F. Structure and navigation

- [ ] Every bundled file is linked directly from `SKILL.md` (one level deep, no chains)
- [ ] Each reference carries a stated condition for when to read it
- [ ] Reference files over ~100 lines open with a table of contents
- [ ] Filenames describe contents (`form-validation-rules.md`, not `doc2.md`)
- [ ] Multi-domain content is split by domain so irrelevant material never loads
- [ ] Critical instructions appear early rather than buried mid-file

## G. Scripts and tools

Skip if the skill ships no code.

- [ ] Scripts handle their own error conditions instead of failing for the agent to figure out
- [ ] Error messages say what went wrong, what was expected, and what to try
- [ ] No unexplained constants — every timeout, retry count, and threshold is justified in a comment
- [ ] No interactive prompts; all input via flags, env vars, or stdin
- [ ] `--help` documents flags and shows usage examples
- [ ] Output is structured (JSON/CSV) with data on stdout and diagnostics on stderr
- [ ] Output size is bounded or paginated
- [ ] Operations are idempotent where the agent might retry
- [ ] Destructive operations have a dry-run or confirmation flag
- [ ] Required packages are listed and confirmed available in the target runtime
- [ ] Instructions say clearly whether to execute or read each script
- [ ] MCP tools are referenced by fully qualified name (`ServerName:tool_name`)

## H. Safety audit

Apply to your own skills and, with more suspicion, to any skill from outside.

- [ ] Every instruction is consistent with the stated purpose — nothing surprising
- [ ] No obfuscated content: no base64-encoded commands, no unusual Unicode, no
      `curl ... | bash` from unpinned sources
- [ ] No hidden instruction-like text (HTML comments, zero-width characters, "ignore previous
      instructions", fake system messages)
- [ ] No hardcoded secrets, API keys, or credentials
- [ ] No instructions to echo, print, or transmit credentials
- [ ] Network calls, if any, are disclosed and go to pinned, named, trustworthy endpoints
- [ ] No runtime fetching of instructions the agent will then follow
- [ ] If the skill fetches external content, it tells the agent to treat that content as data
      rather than as instructions
- [ ] No instruction to disable, bypass, or ignore safety mechanisms
- [ ] Requested tool access is proportionate to the task

Weight this section heavily for anything installed from a public directory. Independent scanning of
public skill marketplaces has found a meaningful share carrying critical issues — malware links,
obfuscated exfiltration, embedded secrets — and skills run with the agent's full permissions, so a
bad one is not sandboxed away from your credentials or filesystem.

## I. Maintainability

- [ ] Someone other than the author could edit this without archaeology
- [ ] `metadata.version` set if the skill will be distributed
- [ ] `compatibility` set if the skill needs specific tools, packages, or network access
- [ ] Test cases stored with the skill (e.g. `evals/evals.json`) so future edits are checkable
- [ ] No `README.md` inside the skill folder — human docs belong at the repo root, since everything
      inside the folder is agent-facing
- [ ] Deprecated guidance is removed or clearly marked, not left ambiguous

## Scoring and verdict

Section A is pass/fail. For B-I, count fails weighted by consequence:

- **Blocker** — won't load, won't trigger, or fails the safety audit
- **Major** — will produce wrong or inconsistent output, or wastes significant context
- **Minor** — style, polish, future maintenance

Give a verdict in one line ("ready to test", "fix 2 blockers first", "do not install"), then the
findings. Resist listing twenty minor items alongside one blocker — it buries the thing that matters.

## Reviewing someone else's skill

Read every bundled file, not just `SKILL.md`: scripts, assets, reference docs, and anything binary.
Check what the skill does against what its description claims, and treat any gap as the finding.
Pay particular attention to setup and prerequisite sections, which are where installation-time
attacks hide, and to anything the skill downloads or executes.

If the skill fetches from external sources, remember that its behavior can change after you review
it, without the skill file changing at all.
