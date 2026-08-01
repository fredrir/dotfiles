# AI-Slop Tells Catalog

The full reference behind `SKILL.md`. Each entry gives a **detection signal** (what to look for), **why it reads as AI**, and a **fix** — with code for plain HTML/CSS and for React + Tailwind + shadcn where it helps. Read the category you need; you do not have to read top to bottom.

Every fix rests on one principle from `SKILL.md`: **the tell is the *unchosen default*, not the element.** Do not blanket-ban gradients, Inter, cards, or centered heroes. Replace a reflexive default with a real decision grounded in the subject. And after fixing, re-check that you did not swap one median for another (see [§9](#9-verify-and-dont-over-correct)).

## Contents

1. [Color & gradient](#1-color--gradient)
2. [Typography](#2-typography)
3. [Layout & structure](#3-layout--structure)
4. [Components & surfaces](#4-components--surfaces)
5. [Iconography](#5-iconography)
6. [Motion & interaction](#6-motion--interaction)
7. [Copy & content](#7-copy--content)
8. [Systemic fixes](#8-systemic-fixes)
9. [Verify (and don't over-correct)](#9-verify-and-dont-over-correct)

---

## 1. Color & gradient

**The indigo/violet→blue gradient.** *Signal:* `bg-gradient-to-*` running purple/indigo into blue or pink, most often behind a hero headline. *Why:* the single loudest 2026 tell; it traces to Tailwind's `indigo-500` default and the training-data feedback loop around it. *Fix:* if the design wants a gradient, build one from the subject's own palette (two adjacent brand hues, or a duotone from a brand photo), and give it a job (depth, a focal glow) rather than decoration. Otherwise use a flat, deliberate ground color. Never the default purple→blue.

**Gradient text on headings (`bg-clip-text`).** *Signal:* `bg-clip-text text-transparent` on an `<h1>`. *Why:* reads as a reflex, and usually hurts legibility and hierarchy. *Fix:* solid high-contrast headline color; if you want emphasis, use weight, size, or a single accent word in a real accent color — not a rainbow fill.

**Untouched framework palette.** *Signal:* `indigo-600`, `slate-900`, `emerald-500`, or shadcn's `zinc`/`slate`/`gray` used verbatim across the UI. *Why:* these tokens appear in a billion tutorials; using them unedited signals no color decision was made. *Fix:* replace the palette wholesale — see [§8](#8-systemic-fixes). Build 4–6 named values from the subject.

**Timid, even palette with no anchor.** *Signal:* several mid-saturation colors of equal visual weight, no dominant color, no real accent. *Why:* averaging produces a "safe," weightless spread. *Fix:* choose one dominant color that carries the brand and one accent that earns attention; let neutrals recede. A palette should have a hierarchy, not a democracy.

**Pure `#fff` / `#000` with no depth.** *Signal:* `bg-white`/`bg-black` everywhere, flat, no layering. *Why:* the model rarely commits to a considered neutral. *Fix:* pick a specific off-white or near-black (a tuned neutral with a hint of the brand hue), and build 2–3 elevation levels so surfaces read as layered, not flat.

**The colored 3–4px left-border strip.** *Signal:* `border-l-4 border-indigo-500` on cards/callouts. *Why:* an oddly reliable AI tell — a decoration the model adds for "impact." *Fix:* if you need to distinguish a callout, use a considered background tint, spacing, or a real icon; if you keep an accent bar, make it part of a deliberate system, not a sprinkle.

---

## 2. Typography

**Unchosen Inter / Roboto / system.** *Signal:* `font-family: Inter, system-ui` (or Tailwind's default sans) with no display face. *Why:* Inter is excellent, which is exactly why it is the default and now reads as "nobody chose a font." *Fix:* pair a display face with a point of view against a clean body face, and set an intentional type scale. See [§8](#8-systemic-fixes) for wiring.

**The "safe" non-choice fonts.** *Signal:* Space Grotesk, Geist, or Poppins dropped in as the whole type system. *Why:* these became the reflexive "de-slop" swap, so they now read as their own default. *Fix:* choose for the subject. A characterful serif, a grotesk with real personality, a mono paired intentionally — anything that expresses the brief rather than the current escape hatch.

**Hierarchy by boldness alone.** *Signal:* everything is the body face; headings are just `font-bold`. *Why:* no real type scale. *Fix:* establish size, weight, tracking, and leading steps so hierarchy is legible at a glance without relying on bold.

**Invented over-hierarchy in the hero.** *Signal:* four or five distinct type styles stacked in one hero (logotype + H1 + subhead + label + decorative bit). *Why:* the model adds a "clever" extra style and creates visual noise instead of clarity. *Fix:* one clear headline, one supporting line, one action. Remove styles until the hierarchy parses instantly.

**Decorative all-caps labels everywhere.** *Signal:* `uppercase tracking-widest` eyebrows on every section. *Why:* used as reflexive structure rather than meaning. *Fix:* keep an eyebrow only where it encodes something true (a real category/step); vary or drop the rest.

**Monospace "for the hacker vibe."** *Signal:* body or headings in mono with no functional reason. *Why:* aesthetic cosplay the model reaches for. *Fix:* reserve mono for what is actually code/data/technical; choose a real display face for everything else.

**One serif-*italic* accent word.** *Signal:* a single italic-serif word dropped into an otherwise-sans line for "elegance." *Why:* a stock move. *Fix:* if you want emphasis, make it a deliberate, repeated part of the type system, not a lone garnish.

---

## 3. Layout & structure

**The canned full-page skeleton.** *Signal:* hero → exactly three feature cards → logo/"trusted by" strip → three-tier pricing (middle plan raised) → FAQ accordion → footer, in that order, shipped as-is. *Why:* it is the shadcn/Tailwind-demo and Vercel-template structure, learned as "how a landing page looks" — not what this product needs. *Fix:* start from *this* product's actual story and pick the sections and order it needs. Cut sections that carry nothing. Lead with the single most characteristic thing in the subject's world, whatever form that takes.

**Centered hero: subhead, two buttons, badge above the H1.** *Signal:* `min-h-screen flex items-center justify-center text-center`, a pill badge directly above the headline, primary + ghost button pair. *Why:* the default hero composition. *Fix:* try an asymmetric or editorial hero — content offset, a real image or live element bleeding past the text column, a single decisive action. Code in [§8](#8-systemic-fixes).

**Exactly three cards in a row, by reflex.** *Signal:* `grid-cols-3` of near-identical feature cards. *Why:* the three-column grid Tailwind tutorials used to demo layout. *Fix:* let the content decide the count and rhythm; break the triptych — a 2+1, a staggered list, a comparison, or differently-weighted items. Breaking this one reflex de-slops a page more than almost anything else.

**Horizontal stat banner.** *Signal:* a row of `10k+ / 99.9% / 24/7` big-number-over-label stats. *Why:* stock "social proof." *Fix:* use only real numbers, and present them where they support a claim, in a form that fits the design — not a default four-up band. Vague round numbers read as invented.

**The "1 · 2 · 3" numbered step row.** *Signal:* numbered markers (01/02/03) on items that are not actually a sequence. *Why:* numbering used as decoration. *Fix:* number only genuine ordered steps; otherwise drop the markers. Structural devices should encode something true.

**Everything full-width and vertically stacked.** *Signal:* every section is a full-bleed centered band. *Why:* the safest structure to generate. *Fix:* vary rhythm and containment — asymmetry, overlap, an element that breaks the grid — so composition reads as chosen.

**Inconsistent visual language between sections.** *Signal:* different color treatments, spacing conventions, or component styles across sections. *Why:* sections generated separately without shared constraints. *Fix:* establish the token system *before* generating and derive every section from it; do not patch inconsistencies after.

---

## 4. Components & surfaces

**The default card on every surface.** *Signal:* `rounded-2xl shadow-lg p-6` (often `+ backdrop-blur`) on essentially every box. *Why:* the shadcn default card, unedited, everywhere. *Fix:* pick one surface vocabulary (radius OR border OR shadow OR flat) and hold it. React example:

```jsx
// Decide the vocabulary ONCE; variants share it. No per-component drift toward the default.
export function Surface({ variant = "base", children }) {
  const styles = {
    base:  "border border-ink-200 bg-paper",       // borders + contrast, no shadow
    inset: "bg-ink-50",                              // flat, differentiated by tone
    lift:  "border border-ink-200 bg-paper shadow-[0_1px_0_theme(colors.ink.200)]", // one intentional shadow
  };
  return <div className={`${styles[variant]} p-5`}>{children}</div>;
}
```

**Reflexive glassmorphism.** *Signal:* `backdrop-blur` + translucent white on cards/navbars without cause. *Why:* a default "premium" effect. *Fix:* use blur only where layering over real content justifies it; otherwise solid, considered surfaces.

**Cardocalypse (cards nested in cards).** *Signal:* every block boxed; cards inside cards inside sections. *Why:* the model boxes everything to feel organized. *Fix:* use whitespace and typographic grouping to structure content; reserve a card for a genuinely discrete, liftable unit.

**Flat 1px gray border on everything.** *Signal:* `border border-gray-200` on all surfaces uniformly. *Why:* the neutral default outline. *Fix:* make containment a deliberate choice — tint, spacing, or a considered border that fits the brand — not a blanket hairline.

**Default blue buttons.** *Signal:* `bg-indigo-600 hover:bg-indigo-700` (or blue-600). *Why:* the Tailwind/shadcn default action color. *Fix:* derive the primary action color from the brand palette; give the button a considered shape, weight, and state design. Ensure hover makes it more prominent, not less.

**The FAQ accordion as reflex.** *Signal:* an accordion tacked on at the bottom because the template has one. *Why:* structural muscle memory. *Fix:* include an FAQ only if there are real recurring questions; present them in whatever form serves the reader.

---

## 5. Iconography

**Interchangeable thin-line icons (default Lucide set).** *Signal:* generic outline icons that could illustrate any product, one per feature. *Why:* the default icon set, learned as "how features look." *Fix:* choose an icon set that fits the brand's weight (consider Phosphor, Heroicons, Radix, or a custom set), use it consistently, and make icons specific to the actual feature. Icons should clarify, not fill a slot.

**Icon-in-a-rounded-square on every feature.** *Signal:* each feature led by an icon inside a tinted `rounded-xl` square. *Why:* the default feature-card garnish. *Fix:* drop the container, or design a treatment that means something; often the icon alone (or none) is stronger.

**Emoji used as icons or bullets.** *Signal:* 🚀 ✨ 🔒 standing in for a real icon system or list markers. *Why:* a hallmark of unedited generated UI; reads as amateur. *Fix:* use a real SVG icon set (see above). Reserve emoji for genuinely conversational contexts, never as the interface's icon language.

---

## 6. Motion & interaction

**Everything fades in on scroll.** *Signal:* a blanket scroll-reveal on every section. *Why:* the default "add some motion" move. *Fix:* animate deliberately and sparingly; one orchestrated moment (a considered page-load sequence, a single meaningful reveal) usually lands harder than uniform fades. Sometimes none is right.

**The cursor-trailing line / element.** *Signal:* a line or shape following the pointer down the page for no reason. *Why:* motion for its own sake. *Fix:* remove it, or replace with an interaction that serves the content.

**Hover states that make things recede.** *Signal:* buttons/links that fade or lose contrast on hover. *Why:* an inverted interaction the model produces surprisingly often — the opposite of what feedback should do. *Fix:* hover/focus should make a control *more* prominent and clearly actionable; design visible keyboard focus too.

**Scattered micro-effects with no theme.** *Signal:* assorted unrelated animations sprinkled around. *Why:* no motion direction. *Fix:* decide a small, coherent motion vocabulary (timing, easing, what moves and why) and apply it consistently; respect `prefers-reduced-motion`.

---

## 7. Copy & content

Visual tells travel with verbal ones; weightless copy trips the same uncanny reaction even in non-technical viewers. Rewrite anything that could describe ten thousand other products.

**Inflated opener verbs.** *Signal:* headlines/sections starting with "Elevate, Unlock, Empower, Transform, Supercharge, Unleash, Revolutionize." *Fix:* say the specific thing the product does, in plain words, ideally with a concrete outcome.

**Weightless slogan copy.** *Signal:* "Build faster. Ship smarter.," "Built for modern teams," "The future of X." *Why:* the textual purple gradient — grammatically fine, says nothing. *Fix:* one specific, verifiable claim only this product could make.

**Two-abstract-noun feature titles.** *Signal:* "Seamless Integration," "Powerful Analytics," "Enterprise Security." *Fix:* name the concrete capability and, where possible, a number or specific.

**"Get Started" as the only CTA.** *Signal:* every action is a generic "Get Started"/"Learn More." *Fix:* name what actually happens ("Import your CSV," "Book a 20-min call"); keep the verb consistent through the flow (a "Publish" button produces a "Published" toast).

**Placeholder names, orgs, and round numbers.** *Signal:* "John Doe," "Acme Corp," "Jane Smith," and metrics like exactly `10,000+` / `99%` / `24/7`. *Why:* obvious filler and invented-looking figures. *Fix:* use realistic, varied names and organic, specific numbers; if you do not have real figures, use a specific claim instead of a rounded one.

**Copy that no real person would say.** *Signal:* not a single sentence with a human cadence. *Fix:* include at least one line that sounds like an actual person wrote it. Reading the copy aloud surfaces the robotic ones fast.

Note: the em dash is **not** an AI tell — do not strip em dashes or hunt any single token as "proof." The fingerprint is the cluster of hollow, interchangeable phrasing, not one character.

---

## 8. Systemic fixes

Fix at the source so defaults cannot reappear. These are cross-cutting and worth more than per-instance patches.

### 8.1 Replace the palette so defaults don't compile

Override `colors` (do **not** `extend`) so `bg-indigo-600` and friends stop existing — the build fails on a default, forcing everyone onto the custom palette. Build the scale from the subject, not from `indigo`.

```js
// tailwind.config.js
export default {
  theme: {
    colors: {
      transparent: "transparent",
      current: "currentColor",
      // A tuned neutral (not #fff/#000) + a real dominant + one accent, all subject-derived.
      paper: "#faf8f3",
      ink:   { 50: "#f2efe8", 200: "#d9d3c7", 500: "#4a453b", 900: "#1c1a15" },
      brand: { 500: "#2f6f6a", 700: "#1f4f4b" },   // dominant — replace with the subject's own
      accent:{ 500: "#e0603a" },                    // one accent that earns attention
    },
    fontFamily: {
      display: ['"YourDisplayFace"', "serif"],
      sans:    ['"YourBodyFace"', "system-ui", "sans-serif"],
    },
  },
};
```

The hex values above are placeholders to show the *shape* (tuned neutral + dominant + single accent). Derive real ones from the brief. Do not copy these — and note that fleeing blue-purple toward warm terracotta/clay is itself a recognized default (see [§9](#9-verify-and-dont-over-correct)).

For plain CSS, do the same with custom properties and use only those:

```css
:root {
  --paper:#faf8f3; --ink-900:#1c1a15; --ink-500:#4a453b; --ink-200:#d9d3c7;
  --brand-500:#2f6f6a; --accent-500:#e0603a;
}
/* Reach only for the tokens; never a raw default hue. */
```

### 8.2 Break the layout grammar

Replace the centered stack with an asymmetric composition. This one change reads as human intent more than any other.

```css
/* Off-center hero: content sits in a column, the visual bleeds past it. */
.hero{
  display:grid;
  grid-template-columns:minmax(1.5rem,1fr) minmax(0,36rem) minmax(0,1fr);
  align-items:end; min-height:80vh;
}
.hero__content{ grid-column:2; padding-block:4rem; }   /* not centered across the page */
.hero__art{ grid-column:2 / -1; align-self:stretch; }  /* extends beyond the text column */
```

Then let feature content break the three-card triptych: a 2+1 split, a staggered vertical list, a comparison, or differently-weighted items sized to their real importance.

### 8.3 Pick one component vocabulary and enforce it

Decide once — radius vs. border vs. shadow vs. flat — and route all surfaces through a single component (see the `Surface` example in [§4](#4-components--surfaces)) so nothing drifts back to `rounded-2xl shadow-lg`. One radius scale, one shadow (if any), one border treatment, applied consistently.

### 8.4 Rewrite copy on a checklist

Before showing anyone, confirm: no inflated opener verb; no two-abstract-noun feature title; at least one concrete claim with a real number; at least one sentence a real person would say; CTAs name what actually happens; no "John Doe"/"Acme"/round-number filler. Read it aloud.

### 8.5 Motion discipline

Define a small motion vocabulary (one or two easings, consistent durations, a rule for what animates and why). Prefer a single orchestrated moment over blanket scroll-reveals. Ensure hover/focus increases prominence and that `prefers-reduced-motion` is honored.

### 8.6 Enforce in review / CI (optional but effective)

Make the defaults un-shippable. Options: replacing `colors` (8.1) already breaks default color classes at build time; add a lint rule or a grep in CI that fails on the loud patterns. Keep the list to the genuinely loud tells so it does not fight real design work.

```js
// Simplified ESLint-style check: fail the build on default-AI class patterns.
const banned = [
  /bg-(indigo|violet|purple)-\d+/,
  /from-(purple|indigo|violet)-\d+.*to-(blue|pink)-\d+/, // the gradient
  /rounded-(2xl|3xl)/,
  /shadow-(lg|xl|2xl)/,
];
// Report any string literal (className) matching a banned pattern.
```

Tune to the project — if the chosen vocabulary legitimately uses, say, one shadow token, exempt it. The goal is to block reflexes, not to handcuff the design.

---

## 9. Verify (and don't over-correct)

After fixing, confirm you did not simply relocate to a different median:

- **Re-run the Quick scan** from `SKILL.md`. Loud tells should be gone, not merely recolored.
- **Check you didn't adopt a newer default.** Swapping Inter→Space Grotesk/Geist, or blue-purple→cream-serif-terracotta (warm accents near `#D97757`), is trading one anonymous look for another. `frontend-design` flags the same three AI-clustered looks (warm-cream + serif + terracotta; near-black + single acid accent; broadsheet with hairline rules and zero radius) — a fix that lands in one of those has not solved the problem. Every choice should trace to *this* subject.
- **Confirm consistency.** One palette, one type system, one surface vocabulary, one motion vocabulary, applied across every section — inconsistency between sections is itself a tell.
- **Keep the quality floor.** Responsive to mobile, visible keyboard focus, reduced motion respected, legible contrast. De-slopping never trades away accessibility.
- **Honor the brief.** If the user explicitly asked for a gradient, Inter, a centered hero, or one of the clustered looks, deliver it deliberately and well. The brief's words win; the target was always the *reflexive* default, never the element.

The test to end on: *does this page say something only this product could say, and look like something only this brand would ship?* If yes, it has been designed, not averaged.
