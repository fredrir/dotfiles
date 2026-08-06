# Instruction patterns

## Contents
- Choosing a pattern
- Pattern: Gotchas
- Pattern: Output template
- Pattern: Worked examples
- Pattern: Workflow checklist
- Pattern: Validation loop
- Pattern: Plan-validate-execute
- Pattern: Conditional branch
- Pattern: Domain split
- Pattern: Default with escape hatch
- Anti-patterns

## Choosing a pattern

Match the pattern to the failure you're preventing. Most skills need two or three, not all nine.

| The agent... | Use |
|---|---|
| makes assumptions that are reasonable but wrong here | Gotchas |
| produces the right content in the wrong shape | Output template |
| gets the shape right but the register or detail level wrong | Worked examples |
| skips steps in a long process | Workflow checklist |
| ships work with errors that were checkable | Validation loop |
| makes errors expensive to undo | Plan-validate-execute |
| applies one path's instructions to a different path | Conditional branch |
| loads context irrelevant to the task at hand | Domain split |
| dithers between equivalent approaches | Default with escape hatch |

## Pattern: Gotchas

The highest-value content in most skills. These are environment-specific facts that defy reasonable
assumptions — corrections to mistakes the agent *will* make otherwise, not general advice.

```markdown
## Gotchas

- The `users` table uses soft deletes. Queries need `WHERE deleted_at IS NULL` or results
  include deactivated accounts.
- The same value is `user_id` in the database, `uid` in the auth service, and `accountId`
  in the billing API.
- `/health` returns 200 whenever the web server is up, even with the database down.
  Use `/ready` for real service health.
```

Keep these in `SKILL.md` rather than a reference file. A reference file only gets read when the
agent recognizes it needs it — and by definition it won't recognize a trap it doesn't know exists.

Every time you correct the agent while using a skill, that correction is a gotcha. Adding them as
they arise is the cheapest way to improve a skill over time.

## Pattern: Output template

More reliable than describing a format in prose, because models pattern-match against concrete
structures well. Signal strictness explicitly — the same template means different things with
"use exactly" versus "adapt as needed".

```markdown
## Report structure

Use this template, adapting sections to the specific analysis:

# [Analysis Title]

## Executive summary
[One paragraph]

## Key findings
- Finding with supporting data

## Recommendations
1. Specific actionable recommendation
```

Short templates go inline. Long ones, or ones needed only in some cases, go in `assets/` and get
referenced with a condition.

## Pattern: Worked examples

When output quality depends on register, tone, or granularity that prose can't pin down, show
input/output pairs. Two or three examples convey more than a paragraph of description.

```markdown
## Commit message format

**Example 1**
Input: Added user authentication with JWT tokens
Output:
feat(auth): implement JWT-based authentication

Add login endpoint and token validation middleware

**Example 2**
Input: Fixed bug where dates displayed incorrectly in reports
Output:
fix(reports): correct date formatting in timezone conversion

Use UTC timestamps consistently across report generation
```

Vary the examples along the dimension that matters. Three near-identical examples teach less than
two that differ in a meaningful way.

## Pattern: Workflow checklist

For multi-step processes with dependencies or validation gates. Having the agent copy the checklist
into its response and tick items keeps long processes on track and makes skipped steps visible to
the user.

```markdown
## Form processing workflow

Copy this and check off as you go:

- [ ] 1. Analyze the form — run `scripts/analyze_form.py`
- [ ] 2. Create field mapping — edit `fields.json`
- [ ] 3. Validate mapping — run `scripts/validate_fields.py`
- [ ] 4. Fill the form — run `scripts/fill_form.py`
- [ ] 5. Verify output — run `scripts/verify_output.py`

**Step 1: Analyze the form**
`python scripts/analyze_form.py input.pdf` extracts fields and locations to `fields.json`.
...
```

This works equally well without code — a research or review workflow benefits from the same
structure.

## Pattern: Validation loop

Do the work, check it, fix, recheck. This single pattern improves output quality more than almost
anything else, because it converts a one-shot generation into an error-correcting process.

```markdown
## Editing workflow

1. Make your edits
2. Validate: `python scripts/validate.py output/`
3. If validation fails:
   - Read the error carefully
   - Fix the issues
   - Validate again
4. Only proceed once validation passes
```

The validator doesn't have to be code. A style guide or checklist works — the agent reads it and
compares its own output against it. Code is more reliable where the check is mechanical; a document
works where the check needs judgment.

## Pattern: Plan-validate-execute

For batch or destructive operations. The agent writes an intermediate plan in a structured format,
a validator checks the plan against a source of truth, and only then does it execute.

```markdown
## PDF form filling

1. Extract fields: `python scripts/analyze_form.py input.pdf` → `form_fields.json`
2. Create `field_values.json` mapping each field name to its value
3. Validate: `python scripts/validate_fields.py form_fields.json field_values.json`
4. If validation fails, revise `field_values.json` and re-validate
5. Fill: `python scripts/fill_form.py input.pdf field_values.json output.pdf`
```

Step 3 is the whole pattern. Errors get caught before anything is touched, the plan is cheap to
revise, and verification is objective rather than a matter of the agent checking its own reasoning.

Make the validator verbose and specific. `"Field 'signature_date' not found — available fields:
customer_name, order_total, signature_date_signed"` lets the agent self-correct in one step;
`"invalid field"` costs a turn of guessing.

## Pattern: Conditional branch

Route explicitly at decision points so the agent doesn't blend two incompatible procedures.

```markdown
## Document modification

1. Determine the type:
   - **Creating new content?** → Creation workflow
   - **Editing existing content?** → Editing workflow

2. **Creation workflow**: build from scratch with docx-js, export to .docx
3. **Editing workflow**: unpack, modify XML directly, validate each change, repack
```

When branches get long, move each into its own file and have `SKILL.md` do only the routing. That
also stops the unused branch from consuming context.

## Pattern: Domain split

When a skill covers several domains that are rarely needed together, organize by domain so only the
relevant slice loads.

```
bigquery-analysis/
├── SKILL.md              # navigation + shared workflow
└── references/
    ├── finance.md        # revenue, ARR, billing
    ├── sales.md          # pipeline, opportunities
    └── product.md        # API usage, adoption
```

```markdown
## Available datasets

**Finance** — revenue, ARR, billing → `references/finance.md`
**Sales** — opportunities, pipeline → `references/sales.md`
**Product** — API usage, adoption → `references/product.md`
```

A question about revenue loads `finance.md` and nothing else. The rest costs zero tokens.

## Pattern: Default with escape hatch

Give one recommendation and name alternatives only where they're genuinely needed. Presenting
options as equals makes the agent deliberate instead of act.

```markdown
Use pdfplumber for text extraction:

    import pdfplumber

For scanned PDFs requiring OCR, use pdf2image with pytesseract instead.
```

## Anti-patterns

**Explaining what the agent knows.** Three paragraphs on what a PDF is. Cut anything that fails
"would it get this wrong without this?"

**A menu of options.** *"You can use pypdf, or pdfplumber, or PyMuPDF, or pdf2image..."* Pick one.

**Nested references.** `SKILL.md` → `advanced.md` → `details.md`. Files reached through a chain get
partially read, so the agent acts on a fragment. Keep everything one level from `SKILL.md`.

**Unconditional pointers.** *"See references/ for details"* gives the agent no basis for deciding
whether to read. State the condition: *"read `references/api-errors.md` if the API returns
non-200"*.

**Capitalized shouting.** ALWAYS, NEVER, and MUST used in place of explanation. Reframe as "do X,
because Y causes Z" — a model that understands the purpose handles the cases you didn't foresee.
(Strengthening the wording of one specific rule that keeps getting dropped is a reasonable targeted
fix; a file full of capitals is not.)

**Dated content.** *"Before August 2025, use the old API."* Put superseded material in a collapsed
"Old patterns" section instead.

**Drifting terminology.** Mixing "field", "box", "element", and "control" for one concept. Pick one
word and hold it.

**The specific answer instead of the method.** *"Join orders to customers on customer_id, filter
region = 'EMEA', sum amount"* only helps for that exact question. Teach how to construct the query
from the schema.

**Windows paths.** `scripts\helper.py` breaks on Unix. Forward slashes everywhere.

**Deferring errors to the agent.** A script that raises on a missing file, expecting the agent to
sort it out. Handle the condition, or fail with a message that says what to do next.

**Magic numbers.** `TIMEOUT = 47`. If the author can't say why, the agent certainly can't. Justify
every constant in a comment, or drop it.

**Assuming installed packages.** *"Use the pdf library."* State the dependency, confirm it exists in
the target runtime, and remember that some environments have no network access and no runtime
installation at all.

**Bare MCP tool names.** `bigquery_schema` may not resolve with multiple servers connected. Use
`ServerName:tool_name`.
