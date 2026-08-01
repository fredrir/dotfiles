---
name: humanize
description: Rewrite AI-generated or AI-assisted text so it reads like it was written by a person — cutting puffery, hollow analysis, formulaic structure, and the vocabulary and formatting tics LLMs overuse. Use this skill whenever the user asks to "humanize," "de-slop," "de-AI," or "make this sound less like ChatGPT/AI," and also whenever they ask why their text sounds robotic, ask for a draft to be made to sound more natural or more like them, ask for an AI-sounding blog post/essay/email/report to be edited, or hand over a draft they admit was AI-written and want fixed. Trigger even when the words "AI" or "humanize" never appear — a request to make writing "less generic," "less corporate," "punchier," or "sound like a human wrote it" counts.
---

# Humanize

Rewrite text so it stops reading like model output.

## The core idea

The tells people notice — *delve*, *tapestry*, *serves as a testament to*, em dashes everywhere — are symptoms. The disease is that LLM prose **regresses to the mean**: it replaces specific, odd, load-bearing facts with generic, positive, important-sounding ones. "Invented the first train-coupling device" becomes "a revolutionary titan of industry." The subject gets simultaneously vaguer and more exalted.

This matters for how the rewrite works. Swapping banned words for synonyms produces text that is still hollow, just harder to spot. That's not the goal. **Fix the substance first, then the sentences, then the formatting.** If a paragraph says nothing after you strip the puffery, the honest edit is to delete it, not to reword it.

The other half of the job is that human writing has positive traits AI avoids, not just negative ones it overuses. Real writers use *is* and *has*. They hedge. They repeat a word three times because it's the right word. They make flat definitive claims. They write one four-word sentence after a forty-word one. Add these back deliberately — a text scrubbed of AI tics but still uniformly smooth still reads as machine-made.

## Workflow

1. **Read the whole thing first.** Identify what it's actually claiming and which claims are load-bearing versus decorative.
2. **Run the linter** for a fast inventory of surface tells:
   `python scripts/flag.py <file>` (or pipe text on stdin). It reports flagged vocabulary, formatting tics, and a density score. Treat its output as candidates for review, not a hit list — every word on it is a legitimate English word in some context.
3. **Diagnose in layers** — substance, then sentences, then formatting. Sections below.
4. **Rewrite.** Preserve every fact and the author's argument. Cut what has no content.
5. **Self-check** against the checklist at the end.

Ask the user one question up front only if it genuinely changes the output: who's the audience, or is there a sample of their own writing to match. Otherwise just do the work and note assumptions afterward.

## Layer 1 — Substance (fix this first; it's most of the job)

**Manufactured significance.** LLMs bolt importance onto whatever they describe: *marking a pivotal moment*, *reflecting broader trends*, *cementing its place*, *setting the stage for*, *a testament to*. Cut these outright. If the significance is real, state the concrete consequence instead; if you can't name one, there wasn't any.

**Trailing participial analysis.** The `-ing` clause welded to the end of a sentence — *…, highlighting the region's growing importance*, *…, ensuring accessibility for all users*, *…, reflecting a commitment to innovation*. This is the single most recognizable AI move. It reads as analysis but asserts nothing checkable. Delete the clause, or promote it to a real sentence with a subject that actually did something.

**Vague attribution.** *Experts argue*, *observers have noted*, *industry reports suggest*, *critics point out*, *many have described*. Name the person or source, or drop the claim. Also watch inflation: one blog post is not "several publications."

**Puffery.** *Vibrant*, *rich*, *renowned*, *groundbreaking*, *nestled in the heart of*, *boasts a diverse array of*, *commitment to excellence*. Replace with the specific fact that made someone want to praise it, or delete.

**The formulaic ending.** *Despite these challenges…*, a "Challenges" section, a "Future Outlook" section, *In conclusion*, *Overall, X remains…*. LLMs end things by restating them and gesturing hopefully at the future. Real pieces usually just stop, or land on something specific. Cut the summary paragraph; the reader was there.

**Empty hedged filler about gaps.** *While specific details are limited…*, *not widely documented*, *based on available information*, *maintains a low profile*. This is a model narrating its own uncertainty. Cut it. Never replace it by inventing the missing detail.

**Didactic disclaimers.** *It's important to note that*, *it's worth remembering*, *keep in mind that*. Almost always deletable with zero loss; if the caveat matters, state it as a plain sentence.

## Layer 2 — Sentences

**Restore copulas.** LLMs avoid *is* and *are*, substituting *serves as*, *stands as*, *functions as*, *represents*, *marks*, and swapping *has* for *boasts*, *features*, *offers*, *maintains*. Reverse it: "The gallery serves as the association's exhibition space" → "The gallery is the association's exhibition space." Also *refers to* in opening definitions — an article about a thing shouldn't open by defining the phrase.

**Kill negative parallelism.** *Not just X, but Y*. *It's not X — it's Y*. *No X, no Y, just Z*. *X rather than Y*. These stage a misconception nobody held. Use once per piece at most, deliberately, for a real contrast. Otherwise state the positive claim alone.

**Break the rule of three.** LLMs default to triads — three adjectives, three clauses, three bullets. Vary the count. Two items, or five, or one. This applies to lists too: a genuine list is however long the real list is.

**Undo elegant variation.** Models have a repetition penalty, so a study becomes *the research*, then *the analysis*, then *the investigation*, then *the work*. Pick the right noun and reuse it. Repetition of key terms is a strong signal of human writing and it aids comprehension.

**Vary sentence length hard.** AI prose has flat rhythm — most sentences land in the same 15–25 word band. Put a five-word sentence next to a thirty-five-word one. Let a fragment stand. Start a sentence with *And* or *But* where the logic warrants.

**Thin the transitions.** *Additionally* at the start of sentences, *Furthermore*, *Moreover*, *Notably*, *Consequently*. One or two survive; the rest come out. Often the sentence works better with no connective at all.

**Plain verbs.** *Authored* → wrote. *Utilized* → used. *Relocated* → moved. *Attempted* → tried. *Facilitated* → helped. *Passed away* → died. *Ventured into politics* → ran for office. Reach for the shorter word unless the longer one means something different.

**Vocabulary.** See `references/patterns.md` for the full overused-word list by model era. Don't find-and-replace it — check whether each instance is doing work. *Underscore* meaning a line under text is fine; *underscoring the importance of* is not.

## Layer 3 — Formatting

- **Em dashes**: LLMs overuse them and almost always space them — like this. Convert most to commas, parentheses, colons, or a full stop. Keep a couple, unspaced (em) or set per the house style.
- **Bold**: strip emphasis on random phrases mid-paragraph. Bold is for a term being defined or a genuine label, not for "key takeaways" scattering.
- **Inline-header bullets**: `- **Thing:** description` repeated down the page is a chatbot habit. Convert to prose where the items are connected by argument; keep the list only where the content is genuinely enumerable.
- **Title Case Headings**: convert to sentence case unless the publication uses title case.
- **Emoji as decoration** in headings or bullets: remove.
- **Curly quotes and apostrophes**: harmless in typeset prose, but if the surrounding document uses straight quotes, match it. Mixed straight and curly in one document is a tell.
- **Unnecessary tables** with two columns and four rows: usually a sentence.
- **Horizontal rules** (`---`) before every heading: remove.

## Layer 4 — Add back what humans do

Rewriting is not only subtraction. Deliberately include:

- **Superlatives and flat claims**: *the only*, *the first*, *the best*, *nobody else does this*. AI hedges these into mush.
- **Hedges and intensifiers**: *very*, *pretty much*, *perhaps*, *tends to*, *I think*. Formal AI prose strips these out; people use them constantly.
- **Concrete specificity**: names, numbers, dates, the odd detail. **Only if it's already in the source or the user supplied it.** Never invent a statistic, quote, date, or citation to make prose feel grounded — that's the worst possible failure of this skill.
- **A little inefficiency**: *in order to*, *the fact that*, *a part of*. Not everywhere, but perfectly compressed prose in every sentence reads synthetic.
- **An actual opinion or an admitted gap**, where the piece allows it. "I don't know why they did this" is deeply human.
- **Idiom and register mixing**, matched to the venue. A blog post can say "kind of a mess." A report can't.

## Examples

**Manufactured significance + trailing participle**
Before: The institute was established in 1989, marking a pivotal moment in the evolution of regional statistics and reflecting a broader movement toward decentralization.
After: The institute was established in 1989. Catalonia had been pushing for its own statistical service since the transition to democracy.
(*If you don't know the second sentence, write only the first.*)

**Copula avoidance + puffery**
Before: Nestled in the heart of the valley, the town boasts a vibrant cultural scene and serves as a hub for regional commerce.
After: The town is in the valley's center. It has two theaters, a music festival in August, and the region's largest weekly market.

**Vague attribution + rule of three**
Before: Experts argue the policy has been transformative, reshaping incentives, driving innovation, and fostering long-term growth.
After: Economist Ha-Joon Chang argues the policy worked, mainly by making capital cheap for exporters.

**Formulaic ending**
Before: Despite these challenges, the company remains well-positioned for future growth, and ongoing initiatives could further enhance its market position.
After: [delete]

## Self-check before delivering

- Did I remove any fact, or only the packaging around it? (Facts stay.)
- Did I invent anything — a number, a name, a source, a date? (Nothing invented, ever.)
- Does any paragraph still say nothing? (Delete it.)
- Read three consecutive sentences aloud: do they vary in length and shape?
- Is there at least one flat, unhedged claim and one place where I repeated a key noun rather than varying it?
- Would the author recognize their own argument in this?

Then report to the user: the rewritten text, plus a short note on what changed at the substance level and anything you cut because it had no content. If you cut a claim that might be real but unsupported, flag it so they can source it.

## Scope note

This skill improves writing. It is not a tool for passing off machine output as human work where that's prohibited — in academic submissions, journalism, or anywhere disclosure is required, the honest move is to disclose, and no amount of editing changes that. If a user's framing makes it clear that's the goal, say so plainly once, then help them write well anyway. Also don't promise the result will beat a detector; those tools are unreliable in both directions and optimizing for them produces worse prose than optimizing for a reader.

## Reference files

- `references/patterns.md` — full vocabulary lists by model era, model-specific quirks, and the ineffective tells that cause false positives. Read when doing a detailed diagnostic pass or when the user asks *why* something reads as AI.
- `scripts/flag.py` — pattern linter. Run it first on anything longer than a few paragraphs.
