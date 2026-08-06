# Evaluating a skill

## Contents
- Two independent questions
- Part 1: Trigger testing
- Part 2: Output testing
- Writing assertions
- Grading
- Reading the results
- The iteration loop
- Lightweight mode
- Tooling

## Two independent questions

A skill can fail in two unrelated ways, and they need separate tests:

1. **Does it fire when it should?** Fixed by editing the `description`.
2. **Does it produce good output when it fires?** Fixed by editing the body, references, or scripts.

Seeing a skill trigger once tells you nothing about either. Test them separately, or you'll tune
the wrong thing.

## Part 1: Trigger testing

### Build the query set

Aim for ~20 realistic prompts: 8-10 that should trigger, 8-10 that shouldn't. Realistic means what
someone would actually type — file paths, column names, company names, a bit of backstory, casual
phrasing, the occasional typo. `"Format this data"` tests nothing.

**Should-trigger** queries vary along four axes:
- *Phrasing* — formal, casual, abbreviated
- *Explicitness* — some name the domain, some only describe the need
- *Detail* — terse alongside context-heavy
- *Complexity* — single-step alongside multi-step, including tasks where the skill's job is buried
  in a larger chain

The valuable ones are where the skill helps but the connection isn't obvious from the query. If the
query literally asks for what the skill does, any description passes and you've learned nothing.

**Should-not-trigger** queries must be **near-misses** — sharing keywords or concepts but needing
something different. For a CSV analysis skill: `"update the formulas in my Excel budget"` is a good
negative (shares "spreadsheet", needs Excel editing); `"write a fibonacci function"` is worthless
because nothing would trigger on it.

```json
[
  {"query": "my boss sent me a file in ~/Downloads called 'Q4 sales FINAL v2.xlsx' and wants a profit margin column, revenue is col C and costs col D i think", "should_trigger": true},
  {"query": "can you write a python script that reads a csv and uploads each row to our postgres db", "should_trigger": false}
]
```

### One thing that skews everything

Agents consult skills mainly for tasks they can't handle unaided. A trivially simple query may not
trigger a skill even when the description matches perfectly, because the agent just does it. So
keep test queries substantive — `"read file X"` is a bad test case regardless of description
quality, and a description tuned to force triggering on it will over-trigger everywhere else.

### Run and measure

Model behavior is nondeterministic, so run each query ~3 times and compute a **trigger rate**. A
should-trigger query passes above ~0.5; a should-not-trigger query passes below it.

### Avoid overfitting

Split the query set 60% train / 40% validation. Diagnose failures and make edits using **only** the
train set. Keep the split fixed across iterations. Select the final description by validation score
— which is often not the last version you wrote, since later iterations tend to overfit.

When revising: if positives fail, broaden. If negatives fire, add specificity and an explicit
boundary. Never paste the literal keywords from a failed query — find the *category* it represents
and address that. If several rounds of tweaking stall, rewrite the description with a structurally
different framing rather than continuing to nudge. Five iterations is usually enough; beyond that,
suspect the query set rather than the description.

## Part 2: Output testing

### Test cases

Start with 2-3, not twenty. Each needs a realistic prompt, a plain-language description of what
success looks like, and any input files. Include at least one edge case — malformed input, an
unusual request, or a spot where the instructions might be ambiguous.

```json
{
  "skill_name": "csv-analyzer",
  "evals": [
    {
      "id": 1,
      "prompt": "I have monthly sales in data/sales_2025.csv — find the top 3 months by revenue and make a bar chart",
      "expected_output": "A bar chart showing the top 3 months by revenue, with labeled axes and values.",
      "files": ["evals/files/sales_2025.csv"]
    }
  ]
}
```

Store this at `evals/evals.json` inside the skill directory so it travels with the skill and future
edits stay checkable.

### Run against a baseline

Run every case twice: **with** the skill and **without** it (or against the previous version, when
improving an existing skill — snapshot it before editing). The baseline is the whole point. Without
it you can't distinguish "the skill works" from "the model was going to do this anyway".

Each run needs a **clean context**. Leftover state from the authoring conversation hides gaps in
what you actually wrote down — the run succeeds because *you* explained something that never made it
into the file. Use subagents where available, separate sessions where not.

Capture tokens and duration per run. A skill that lifts pass rate 50 points for 13 extra seconds is
clearly worth it; one that doubles token usage for 2 points is not. You can't make that call without
the numbers.

## Writing assertions

Write assertions **after** seeing the first round of outputs. You usually don't know what "good"
means until the skill has run.

Good assertions are objectively checkable: *"the output file is valid JSON"*, *"the chart has
labeled axes"*, *"the report includes at least 3 recommendations"*.

Weak ones fail in two directions — too vague to grade (*"the output is good"*) or so brittle that
correct output with different wording fails (*"uses exactly the phrase 'Total Revenue: $X'"*).

Not everything needs an assertion. Writing style, visual design, whether the output feels right —
these resist decomposition into pass/fail and belong in human review. Forcing assertions onto
subjective qualities produces numbers that measure nothing.

## Grading

For each assertion record pass/fail **plus evidence** that quotes or points at the output. "Y-axis
is labeled 'Revenue ($)' but X-axis has no label" is evidence; "looks fine" isn't.

Require concrete evidence for a pass — don't give benefit of the doubt. If an assertion says
"includes a summary" and the output has a section titled *Summary* containing one hollow sentence,
that's a fail: the label is present, the substance isn't.

Prefer scripts over judgment for anything mechanical (valid JSON, row counts, file exists, correct
dimensions). Scripts are faster, more consistent, and reusable across iterations.

While grading, audit the assertions themselves. Fix the ones that are always-pass, always-fail, or
uncheckable before the next round.

## Reading the results

- **Always passes in both configurations** → the assertion measures nothing the skill contributes.
  Remove or replace it; it inflates the with-skill pass rate.
- **Always fails in both** → the assertion is broken, the case is too hard, or you're checking the
  wrong thing. Fix before iterating.
- **Passes with, fails without** → this is where the skill earns its keep. Work out *which*
  instruction did it, because that tells you what to protect during future edits.
- **Inconsistent across runs** → either a flaky eval or, more often, an ambiguous instruction the
  model reads differently each time. Add an example or tighten that section.
- **Time or token outlier** → read that transcript. There's usually a specific instruction sending
  the agent down a dead end.

Read execution transcripts, not just final outputs. Wasted steps usually mean instructions that were
vague (the agent tried several approaches), instructions that didn't apply to this task (it followed
them anyway), or too many options with no default.

Also watch for the same helper script being written independently across runs — that's a direct
signal to bundle it in `scripts/`.

## The iteration loop

1. Feed failed assertions, human feedback, and transcripts — along with the current `SKILL.md` — to
   a model and ask for proposed changes.
2. Review and apply.
3. Rerun all cases in a fresh `iteration-N+1/` directory, baselines included.
4. Grade, aggregate, review with a human.
5. Repeat.

Three guardrails for the revision step:

- **Generalize.** The skill will run on prompts unlike your test cases. Fix the underlying issue, not
  the specific example. Narrow patches accumulate into an overfitted mess.
- **Keep it lean.** Fewer, better instructions usually beat exhaustive rules. If pass rates plateau
  while the file keeps growing, try *removing* instructions and see whether results hold.
- **Explain the why.** Reasoning-based instructions outperform rigid directives, because a model
  that understands the purpose handles the cases you didn't anticipate.

Stop when the user is satisfied, feedback comes back consistently empty, or iterations stop moving
the numbers.

## Lightweight mode

Full evals are overkill for a personal skill. The honest minimum:

1. Three realistic prompts, run in a fresh session with the skill and without it.
2. Eyeball both outputs side by side. Is the with-skill version actually better?
3. Three near-miss prompts that shouldn't trigger it. Do they?
4. Fix the worst thing you find. Repeat once.

That takes fifteen minutes and catches most of what matters. It is dramatically better than
shipping a draft that has never been run.

## Tooling

Anthropic ships a `skill-creator` skill that automates most of this loop — spawning isolated runs
per test case, grading assertions, aggregating pass rate/time/tokens against a baseline, blind A/B
comparison between versions, description tuning, and an HTML review viewer. It's available in
claude.ai and Cowork, and installable in Claude Code from the official plugin marketplace.

Use it when you want the full quantitative loop. This skill covers design, review, and the
lightweight path; the two compose — draft and audit here, then hand off for automated benchmarking.
