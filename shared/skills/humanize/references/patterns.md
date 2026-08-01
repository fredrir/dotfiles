# Detailed pattern reference

Contents:
1. Overused vocabulary by model era
2. Phrase templates by category
3. Model-specific quirks
4. Machine artifacts (near-certain evidence)
5. Ineffective tells — do not "fix" these
6. Positive markers of human writing

---

## 1. Overused vocabulary by model era

These words spiked in published text after late 2022. One or two mean nothing. High density, especially co-occurring, is the strongest lexical signal there is.

**2023 – mid-2024 (GPT-4 era):** additionally (sentence-initial), boasts, bolstered, crucial, delve, emphasizing, enduring, garner, interplay, intricate/intricacies, key (adjective), landscape (abstract), meticulous/meticulously, pivotal, tapestry (abstract), testament, underscore, valuable, vibrant

**Mid-2024 – mid-2025 (GPT-4o era):** align with, bolstered, crucial, emphasizing, enhance, enduring, fostering, highlighting, pivotal, showcasing, underscore, vibrant

**Mid-2025 onward (GPT-5 era):** emphasizing, enhance, highlighting, showcasing — plus heavy language about notability, coverage, and sourcing (*independent coverage*, *featured in national outlets*, *profiled in*, *maintains an active social media presence*)

**Also common across eras:** encompassing, ensuring, foster, garner, holistic, leverage (verb), myriad, navigate (abstract), plethora, realm, resonate, robust, seamless, unlock, unwavering

Note the caveat: overuse of a word does *not* imply its synonyms are overused. This list is literal.

---

## 2. Phrase templates by category

**Significance and legacy:** stands as / serves as a testament to, plays a crucial/pivotal/vital role, underscores its importance, reflects broader trends, symbolizing its enduring, contributing to the, setting the stage for, marking a shift, a key turning point, the evolving landscape of, left an indelible mark, deeply rooted in, cementing its place, remains a cornerstone

**Superficial analysis (usually as trailing `-ing` clauses):** highlighting…, underscoring…, emphasizing…, ensuring…, reflecting…, symbolizing…, contributing to…, cultivating…, fostering…, encompassing…, enhancing…, offering valuable insights into…, resonating with…

**Promotional:** boasts a, vibrant, rich, profound, showcasing, exemplifies, commitment to, natural beauty, nestled, in the heart of, groundbreaking, renowned, world-class, state-of-the-art, diverse array, wide range of, seamlessly integrates

**Vague attribution:** industry reports suggest, observers have cited, experts argue, some critics argue, many have noted, several sources/publications, it is widely believed, has been described as, such as (introducing a list presented as non-exhaustive when it's actually complete)

**Notability inflation:** independent coverage, national/regional media outlets, trade publications, profiled in, written by a leading expert, maintains an active social media presence, has been featured in

**Endings:** Despite its [praise], X faces several challenges…, Despite these challenges…, "Challenges and Legacy", "Future Outlook", In conclusion, In summary, Overall, X remains…

**Didactic disclaimers:** it's important to note/remember/consider, worth noting, it should be emphasized, may vary depending on

**Knowledge-gap hedging:** as of my last knowledge update, while specific details are limited, not widely available/documented, in the provided sources, based on available information, maintains a low profile, keeps personal details private

**Copula avoidance:** serves as, stands as, functions as, operates as, represents, marks, embodies / boasts, features, maintains, offers, provides / refers to (in a definition) / began his career as, ventured into (instead of *was*)

**Negative parallelism:** not only X but also Y, it's not just X — it's Y, no X, no Y, just Z, X rather than Y, less about X and more about Y

**Collaborative leftovers** (text meant for the user that got pasted in): Certainly!, Of course!, I hope this helps, Would you like me to…, Let me know if…, Here's a detailed breakdown, Here's a template you can customize

---

## 3. Model-specific quirks

- **ChatGPT / DeepSeek:** curly quotation marks and apostrophes, often mixed inconsistently with straight ones. Heavy em dash use, usually spaced.
- **Gemini and Claude:** typically do not use curly quotes; tend to be more concise than ChatGPT and Grok.
- **ChatGPT and Grok:** more likely to zoom out into broader context and significance than Gemini or Claude.
- **Grok:** overuses pseudo-scientific vocabulary — *causal*, *empirical*, *correlate* — and the *X rather than Y* construction. Still leans on *underscore*.
- **Newer models** have been tuned to suppress em dashes and emoji, so their absence proves nothing.
- **American English by default**, regardless of the writer's location or the topic's national ties, unless prompted otherwise.

---

## 4. Machine artifacts (near-certain evidence)

If any of these appear in pasted text, the source is unambiguous. Strip them and check whether the citations they replaced are real.

- ChatGPT: `:contentReference[oaicite:0]{index=0}`, `oai_citation`, `citeturn0search0`, `turn0image0`, `{"attributableIndex":"0-1"}`, and the URL parameter `utm_source=chatgpt.com` or `utm_source=openai`
- Gemini: `[cite: 1]`, `[span_1](start_span)`, `[span_1](end_span)`
- Grok: `grok_card`, `grok_render_citation_card_json`, `referrer=grok.com`
- DeepSeek: lenticular-bracket markers like `【85†L261-269】`
- Perplexity: `[attached_file:1]`, `[web:1]`, URLs containing `ppl-ai-file-upload`
- Copilot: `utm_source=copilot.com`
- Placeholder residue: unfilled `[insert X here]` templates, `access-date=2025-xx-xx`, `<!-- Add if available with citation -->`
- Markdown syntax pasted where it doesn't render, or wrapped in ```` ```wikitext ```` style fences

**Citations deserve special suspicion.** LLM-generated references frequently have valid-looking DOIs pointing to unrelated papers, ISBNs that fail checksum, book citations with no page number, dead links that were never live, and authors who were dead at the purported publication date. When humanizing text that carries citations, tell the user which ones you couldn't verify rather than silently keeping them.

---

## 5. Ineffective tells — do not "fix" these

Editing on these grounds makes the writing worse and catches innocent human prose:

- **Perfect grammar.** Plenty of people write cleanly.
- **Formal or academic vocabulary in general.** Only the specific overused words matter, not "fancy prose" as a class.
- **Mixed casual and formal register.** Common in technical writers, younger writers, and anyone playful.
- **"Bland" or "robotic" feel** as a standalone judgment. LLM output actually skews warm and verbose.
- **Transition words in isolation.** Style guides recommend them.
- **Unsourced content.** Predates LLMs by decades.
- **Em dashes alone.** Many good writers use them heavily. Density plus spacing plus other signals is what matters.
- **Curly quotes alone.** Word, macOS, iOS, and Chicago style all produce them.
- **Non-native-speaker patterns**, including deliberate avoidance of word repetition — taught explicitly in some school systems.

---

## 6. Positive markers of human writing

Observed to be more common in human text than AI text. Use as a target, not a formula.

- Simple *is* / *are* / *has* constructions: *there is a*, *it has a*
- Short plain verbs where a stiff synonym exists: wrote, moved, used, tried, died, helped, got
- Superlative and definitive statements: *one of the best*, *is the only*, *was the first*
- Hedging qualifiers and intensifiers: *very*, *perhaps*, *tends to*, *sort of*, *probably*
- Wordy constructions left in place: *as a result of*, *in order to*, *all of the*, *the fact that*
- Repetition of a key term instead of elegant variation
- Wide variance in sentence length, including fragments
- Concrete, specific, sometimes trivial detail — the thing a person happened to notice
- Willingness to leave a question open rather than resolve it into a tidy summary
