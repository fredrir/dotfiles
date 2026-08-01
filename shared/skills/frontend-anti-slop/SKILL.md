---
name: frontend-anti-slop
description: 'Keep generated frontend UI from looking and feeling AI-generated. Use this whenever building, generating, editing, or reviewing any web UI, landing page, dashboard, marketing page, or component — especially in React/Tailwind/shadcn or plain HTML/CSS — to detect and strip the "AI slop" fingerprint: indigo-to-purple gradients, unchosen Inter, the centered-hero-plus-three-cards template, untouched shadcn/Tailwind defaults, rounded-2xl and shadow-lg on every surface, emoji-as-icons, and "Elevate your workflow / Get Started" copy. Trigger even when the user never says "slop" or "AI-generated" — any request to make UI look less generic, less templated, less like v0/Lovable/Bolt/ChatGPT built it, more distinctive, more custom, or more human-designed counts. Run the quick scan before delivering ANY generated frontend, not only when asked.'
---

# Frontend Anti-Slop

There is a specific look that says "an AI made this": a blue-to-purple gradient, Inter set in a tight weight, a centered hero with two buttons, a row of exactly three rounded cards with thin-line icons, and a "Get Started" button. It reads as machine-made not because any one piece is ugly, but because none of it was *chosen*. This skill helps you detect that fingerprint and remove it — whether you are generating UI from scratch or auditing UI (yours or someone else's) that already has the look.

This is the anti-pattern companion to the `frontend-design` skill. `frontend-design` drives positive aesthetic direction (palette, type, signature, taste). This skill is the audit: the concrete tells to avoid and the mechanical fixes. When building from scratch, let `frontend-design` set the direction and use this skill as the final pass. When handed existing generated code, this skill is the main tool.

## The one idea to internalize: defaults are the tell, not the elements

A model generating UI is not choosing aesthetics — it is predicting the most likely next token from billions of public code samples that are overwhelmingly tutorials, starter templates, and component-library demos. So it returns the *median* of its training data. The median web design since roughly 2019 was Tailwind's default palette; Tailwind's own creator has publicly (half-jokingly) apologized for making buttons default to `indigo-500` years ago and thereby nudging a generation of AI-generated UIs toward indigo. It is a feedback loop: a default becomes a tutorial staple, becomes dominant in training data, becomes the thing every model reaches for first — and every striking example that gets attention feeds back into the next round of training.

The consequence for how you fix things:

- **A gradient is not the problem. An *unchosen* gradient in the exact hue every model defaults to is the problem.** Inter is an excellent typeface; *unchosen* Inter is the tell, because it signals nobody made a typography decision. A card is a fine container; a `rounded-2xl shadow-lg` card on *every* surface because that is the shadcn default is the tell. The fix is never "ban gradients / ban Inter / ban cards." The fix is to make a real decision wherever the model left a default.
- **Generic is worse than ugly.** Ugly is at least memorable — someone clearly chose it. Generic is anonymous, and an anonymous interface fails to build recognition or trust. The median is the most crowded position on the internet. You cannot prompt an averaging machine into being distinctive; you have to *impose* the decisions it would otherwise average away.

## Two modes

### Mode A — Generating UI (the primary case: your own output)

Slop comes from *absence* — from handing the model (or yourself) every taste decision at once and letting each one fill with the median. So decide first, then generate:

1. **Commit to constraints before writing markup.** Lock in a palette (4–6 named hex values built from something true about the subject), a type pairing with an actual point of view, a layout principle, and a copy voice. Derive every color and type value from that system. If you cannot articulate these, that is the real signal — the brand decisions have not been made, and no amount of prompting substitutes for making them. (Use `frontend-design` for this step.)
2. **Generate from the system, not from muscle memory.** Reach for your named tokens, not `indigo-600` / `slate-900` / `rounded-2xl`.
3. **Run the Quick scan (below) before you deliver.** Always — not only when the user asks for it. Treat any generated page as a draft to be pushed past the default, never a finished page to accept.

### Mode B — Auditing existing UI

1. **Detect.** Walk the Quick scan. For a thorough pass, walk `references/tells-catalog.md`.
2. **Prioritize by loudness.** Fix the loud tells first (color, template layout, font) — they do the most damage per pixel. Then the medium tells (card reflex, icons, buttons, copy), then the cosmetic ones (spacing, motion).
3. **Fix at the source, not per-instance.** Change the theme/token/component definition so the default cannot reappear, rather than patching one component. See "Systemic fixes" in the catalog.
4. **Re-scan.** Confirm you did not simply trade one median for another (see Don't over-correct).

## The high-leverage moves

Most of the "AI look" collapses if you make these decisions. Each has full code recipes (plain CSS and React/Tailwind/shadcn) in `references/tells-catalog.md`.

- **Replace the palette wholesale — don't extend it.** Overriding Tailwind's `colors` (rather than `extend`ing) deletes `bg-indigo-600` et al. entirely, so a default literally will not compile and everyone is forced onto the custom palette. Pick colors that are not in the default scale, anchored to the subject.
- **Break the layout grammar.** The centered-hero + three-cards + logo-strip + pricing + FAQ + footer skeleton is the single most template-y thing about AI pages, because it is the shadcn/Tailwind-demo structure. Asymmetry, an off-center composition, an overlapping element, or content that refuses to fit a neat triptych reads as "a human chose this" more than almost any other single change.
- **Pick one component vocabulary and hold it.** Decide once whether surfaces get radius, borders, shadows, or none — then apply it consistently. Kill the reflexive `rounded-2xl shadow-lg backdrop-blur` on every box; let borders and color contrast do the work if that fits the brand.
- **Choose type with a point of view.** Pair a display face with real character against a clean body face; set an intentional scale, weight, and tracking. Unchosen Inter/Roboto/system is the tell.
- **Rewrite the copy.** Weightless, interchangeable marketing copy ("Elevate your workflow," "Build faster. Ship smarter.," "Seamless integration") is the verbal twin of the purple gradient and trips the same uncanny reaction even in non-technical viewers. Replace with specifics only this product could say — real numbers, real nouns, a sentence that sounds like a person wrote it.
- **Use motion with restraint.** Everything-fades-in-on-scroll, a line that trails the cursor for no reason, and hover states that make buttons *recede* are AI motion tells. Prefer one deliberate, purposeful moment over scattered ambient effects.

## Don't over-correct (read this before "fixing")

The most common failure is fleeing one median straight into another:

- **Today's "safe swaps" have themselves become tells.** Swapping Inter for **Space Grotesk** or **Geist**, or fleeing the blue-purple gradient into the **cream/off-white background + high-contrast serif + terracotta-or-warm-clay accent** look (warm accents near `#D97757`), is not a fix — those are now their own recognizable AI-generated clusters (the `frontend-design` skill flags the same three defaults). Trading the median for a newer median leaves you equally anonymous. Distinctiveness comes from a decision grounded in *this* subject, not from picking a different popular non-choice.
- **It is about *unchosen*, not *forbidden*.** If a gradient, Inter, a card, or a centered hero is genuinely right for the brief — and especially if the user asked for it — use it, deliberately and well. The brief's own words always win. The target is the reflexive default, not the element.
- **Don't chase false tells.** The em dash is not an AI tell — it is centuries-old punctuation, and hunting it wastes effort and mangles good prose. Same for any single lint-bait token treated as proof. The fingerprint is the *cluster* appearing regardless of subject, not one character.
- **Match the fix to the brand, and keep the quality floor.** Borderless/flat/no-radius is not universally correct; it is one vocabulary among several. Whatever you choose, keep it responsive to mobile, keyboard-focusable, and reduced-motion-respecting — de-slopping never means dropping accessibility.

## Quick scan

Fast pass usable on its own. Flag every "yes"; loud tells first. Full catalog with fixes: `references/tells-catalog.md`.

**Loud (screams AI on sight):**
- Indigo/violet/purple → blue gradient, especially on a hero or as `bg-clip-text` headline text.
- Untouched Tailwind (`indigo-600`, `slate-900`) or untouched shadcn (`zinc`/`slate`/`gray`) palette; a timid, even palette with no dominant color and no real accent; pure `#fff`/`#000` with no depth.
- Unchosen Inter / Roboto / system — or the "safe" non-choices (Space Grotesk, Geist, Poppins) used as a default; only `font-bold` for hierarchy.
- The template skeleton shipped as-is: centered hero (often with a badge directly above the H1) → exactly three feature cards → logo strip → pricing (middle plan raised) → FAQ accordion → footer.

**Medium (obvious AI smell):**
- `rounded-2xl shadow-lg p-6` (± `backdrop-blur`) on every surface; a flat 1px gray border on every card; a colored 3–4px left-border strip; "cardocalypse" (everything boxed, cards nested in cards).
- Thin-line icons that could illustrate any product (default Lucide set), an icon inside a rounded square on every feature, or **emoji used as icons/bullets**.
- Default blue buttons (`bg-indigo-600 hover:bg-indigo-700`); one serif-*italic* accent word on an otherwise-sans page; all-caps section labels everywhere; decorative monospace "for the hacker vibe."
- Copy tells: openers like "Elevate / Unlock / Empower / Transform / Supercharge"; feature titles that are two abstract nouns ("Seamless Integration"); "Get Started" as the only CTA; placeholder names ("John Doe," "Acme Corp") and suspiciously round numbers; no sentence a real person would actually say.

**Cosmetic (finish it off):**
- Flat, perfectly uniform spacing with no rhythm; hierarchy that is only "bigger text = heading."
- Fade-in-on-scroll on everything, a cursor-trailing line, or hover states that make elements recede; inconsistent visual language between sections (a sign sections were generated separately without shared constraints).

When several loud items are "yes," the page has not been designed — it has been averaged. Impose the decisions in "The high-leverage moves," then re-scan.

## Rules

1. No "·", "—" chars anywhere

2. Titles, subtitles and buttons should be as short, precise, descriptively phrased. "Confirm", "Exit", "Continue" etc. straight to the point.

3. Avoid "-" and "--"

4. Avoid dots and colored dots, circles etc.

5. Mono fonts shouldn't be mixed and used together with other fonts, and its use should be limited

6. Avoid these unichar icons e.g "→"