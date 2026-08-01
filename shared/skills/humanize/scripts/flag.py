#!/usr/bin/env python3
"""
flag.py — inventory the surface patterns that make text read as AI-generated.

Usage:
    python flag.py draft.md
    cat draft.md | python flag.py
    python flag.py draft.md --quiet      # summary only, no line-by-line

Output is a list of CANDIDATES, not verdicts. Every flagged word is a normal
English word in some context. Judge each hit; a high density of hits across
several categories is the real signal, not any single match.

No dependencies beyond the standard library.
"""

import argparse
import re
import sys
from collections import defaultdict

# (label, regex, note) — all matched case-insensitively unless noted
VOCAB = [
    ("ai-vocabulary", r"\b(delve|delving|tapestry|testament|pivotal|meticulous(?:ly)?|intricac(?:y|ies)|intricate|interplay|garner(?:ed|ing)?|bolster(?:ed|ing)?|underscor(?:e|es|ed|ing)|showcas(?:e|es|ed|ing)|foster(?:s|ed|ing)?|encompass(?:es|ed|ing)?|myriad|plethora|realm|holistic|seamless(?:ly)?|robust|unwavering|profound|vibrant|enduring|renowned|groundbreaking|crucial|vital)\b", "high-frequency LLM vocabulary"),
    ("ai-vocabulary", r"\b(leverag(?:e|es|ed|ing)|navigat(?:e|es|ed|ing) the|unlock(?:s|ing)? the|resonat(?:e|es|ed|ing) with|align(?:s|ed|ing)? with|enhanc(?:e|es|ed|ing))\b", "abstract-usage verbs LLMs favor"),
    ("stiff-diction", r"\b(authored|utiliz(?:e|es|ed|ing)|relocat(?:e|es|ed|ing)|attempt(?:ed|ing) to|facilitat(?:e|es|ed|ing)|passed away|commenc(?:e|ed|ing))\b", "prefer wrote/used/moved/tried/helped/died/began"),
]

PHRASES = [
    ("manufactured-significance", r"\b(serv(?:e|es|ed|ing) as a testament|stands? as a|a testament to|plays? a (?:crucial|pivotal|key|vital|significant) role|mark(?:s|ed|ing) a (?:pivotal|significant|key) (?:moment|shift|turning point)|underscor\w+ (?:the|its) (?:importance|significance)|reflect(?:s|ing) (?:a )?broader|setting the stage for|cementing its|left an indelible mark|deeply rooted in|evolving landscape|remains a cornerstone)\b", "cut, or replace with the concrete consequence"),
    ("trailing-participle", r",\s+(?:highlight|underscor|emphasiz|ensur|reflect|symboliz|contribut|cultivat|foster|encompass|enhanc|solidif|cement|showcas|allow|creat|help)\w*ing\b", "the classic bolted-on analysis clause; delete or promote to a real sentence"),
    ("vague-attribution", r"\b(experts? (?:argue|say|note|suggest)|observers? (?:have )?(?:noted|cited)|industry reports?|critics (?:argue|point out|say)|many (?:have )?(?:noted|described|argue)|it is widely (?:believed|regarded|considered)|several (?:sources|publications|outlets)|has been (?:described|hailed) as)\b", "name the source or drop the claim"),
    ("puffery", r"\b(nestled|in the heart of|boasts?|a diverse array|wide range of|state-of-the-art|world-class|rich (?:history|culture|tapestry|tradition)|natural beauty|commitment to (?:excellence|quality|innovation)|seamlessly integrat\w+)\b", "replace with the specific fact behind the praise"),
    ("copula-avoidance", r"\b(serves? as|stands? as|functions? as|operates? as|represents? a|refers to (?:the|a)|features? (?:a|an|four|several)|maintains? (?:a|an)|offers? (?:a|an))\b", "often just means 'is' or 'has'"),
    ("negative-parallelism", r"(not (?:just|only) [^.,;]{2,40}(?:,| but)|it['\u2019]s not [^.,;]{2,40}(?:,|;|\u2014) it['\u2019]s|isn['\u2019]t (?:about|just) [^.,;]{2,40}(?:,|;|\u2014)|no [a-z]+, no [a-z]+, just)", "staged misconception; state the positive claim alone"),
    ("formulaic-ending", r"\b(despite (?:these|its) challenges|in conclusion|in summary|overall,|looking (?:ahead|forward)|remains? well-positioned|continues? to (?:play|serve|shape)|future (?:outlook|prospects))\b", "endings that restate and gesture hopefully"),
    ("didactic-disclaimer", r"\b(it['\u2019]s important to (?:note|remember|consider)|it is important to (?:note|remember)|worth noting|it should be (?:noted|emphasized)|keep in mind that|may vary depending)\b", "usually deletable with zero loss"),
    ("knowledge-gap-hedge", r"\b(as of my (?:last )?(?:knowledge|training)|while specific (?:details|information)|not widely (?:documented|available|reported)|based on available information|in the provided (?:sources|search results)|maintains? a low profile|keeps? personal details private)\b", "model narrating its own uncertainty; cut, never fabricate the gap"),
    ("chat-leftover", r"(^|\n)\s*(certainly!|of course!|sure!|great question|i hope this helps|let me know if|would you like me to|here['\u2019]s a (?:detailed )?(?:breakdown|template|overview))", "correspondence pasted into the document"),
    ("transition-opener", r"(?:^|\n)\s*(?:additionally|furthermore|moreover|notably|consequently|importantly|ultimately)\b", "sentence-initial connective; keep one or two at most"),
]

ARTIFACTS = [
    ("machine-artifact", r"(contentReference|oaicite|oai_citation|citeturn\d|turn\dsearch\d|turn\dimage\d|attributableIndex|grok_card|grok_render_citation|\[cite:\s*\d|start_span|end_span|\[attached_file:|\[web:\d|ppl-ai-file-upload|utm_source=(?:chatgpt\.com|openai|copilot\.com)|referrer=grok\.com|\u3010\d+\u2020)", "unambiguous generator residue — remove, and verify the citation it replaced"),
    ("placeholder", r"(\[(?:insert|your|add|name of)[^\]]{0,40}\]|20\d\d-[xX]{2}-[xX]{2}|<!--\s*add .{0,40}-->)", "unfilled template text"),
]

FORMATTING = [
    ("spaced-em-dash", r"\s\u2014\s", "AI em dashes are usually spaced; convert most to commas or full stops"),
    ("em-dash", r"\u2014", None),
    ("inline-header-bullet", r"(?:^|\n)\s*[-*\u2022]\s+\*\*[^*]{2,60}\*\*\s*:", "chatbot list format; convert to prose where the items connect by argument"),
    ("curly-quote", r"[\u201c\u201d\u2018\u2019]", None),
    ("emoji-decoration", r"[\U0001F300-\U0001FAFF\u2728\u2B50\u2705\u26A1\u2757]", "remove decorative emoji"),
    ("rule-before-heading", r"(?:^|\n)(?:---|\*\*\*|___)\s*\n+#{1,6}\s", "thematic break before every heading"),
]


def sentences(text):
    return [s for s in re.split(r"(?<=[.!?])\s+", text) if s.strip()]


def title_case_headings(text):
    hits = []
    for i, line in enumerate(text.split("\n"), 1):
        m = re.match(r"\s*#{1,6}\s+(.*)", line)
        if not m:
            continue
        words = [w for w in re.findall(r"[A-Za-z][A-Za-z'\u2019-]*", m.group(1))]
        if len(words) < 3:
            continue
        minor = {"a", "an", "the", "and", "or", "but", "of", "in", "on", "at", "to", "for", "with", "as", "by", "from"}
        major = [w for w in words[1:] if w.lower() not in minor]
        if major and all(w[0].isupper() for w in major):
            hits.append((i, m.group(1)))
    return hits


def rule_of_three(text):
    pat = re.compile(r"\b(\w+(?:\s+\w+){0,2})\s*,\s*(\w+(?:\s+\w+){0,2})\s*,\s*and\s+(\w+(?:\s+\w+){0,2})\b")
    return [(m.start(), m.group(0)) for m in pat.finditer(text)]


def line_of(text, pos):
    return text.count("\n", 0, pos) + 1


def scan(text, patterns, results, flags=re.I):
    for label, pattern, note in patterns:
        for m in re.finditer(pattern, text, flags):
            results[label].append((line_of(text, m.start()), m.group(0).strip().replace("\n", " "), note))


def main():
    ap = argparse.ArgumentParser(description="Flag AI-writing patterns in a text file.")
    ap.add_argument("path", nargs="?", help="file to scan; reads stdin if omitted")
    ap.add_argument("--quiet", action="store_true", help="summary counts only")
    args = ap.parse_args()

    text = open(args.path, encoding="utf-8").read() if args.path else sys.stdin.read()
    if not text.strip():
        print("empty input")
        return

    words = len(re.findall(r"\b\w+\b", text))
    results = defaultdict(list)
    scan(text, VOCAB, results)
    scan(text, PHRASES, results)
    scan(text, ARTIFACTS, results, flags=0)
    scan(text, FORMATTING, results, flags=0)

    for ln, head in title_case_headings(text):
        results["title-case-heading"].append((ln, head, "convert to sentence case unless house style says otherwise"))
    for pos, phrase in rule_of_three(text):
        results["rule-of-three"].append((line_of(text, pos), phrase, "vary the count; two or five is fine"))

    sents = sentences(text)
    lengths = [len(re.findall(r"\b\w+\b", s)) for s in sents] or [0]
    mean = sum(lengths) / len(lengths)
    var = sum((n - mean) ** 2 for n in lengths) / len(lengths)
    sd = var ** 0.5
    per1k = lambda n: round(n * 1000 / max(words, 1), 1)

    order = ["machine-artifact", "placeholder", "chat-leftover", "trailing-participle",
             "manufactured-significance", "vague-attribution", "puffery", "copula-avoidance",
             "negative-parallelism", "formulaic-ending", "didactic-disclaimer",
             "knowledge-gap-hedge", "ai-vocabulary", "stiff-diction", "transition-opener",
             "rule-of-three", "title-case-heading", "inline-header-bullet",
             "spaced-em-dash", "em-dash", "curly-quote", "emoji-decoration", "rule-before-heading"]

    print(f"\n{words} words, {len(sents)} sentences\n")

    if not args.quiet:
        for label in order:
            hits = results.get(label)
            if not hits:
                continue
            note = next((n for _, _, n in hits if n), None)
            print(f"[{label}] {len(hits)} hit(s)" + (f" — {note}" if note else ""))
            seen = set()
            for ln, txt, _ in hits[:12]:
                key = txt.lower()
                if key in seen:
                    continue
                seen.add(key)
                print(f"    L{ln}: {txt[:90]}")
            if len(hits) > 12:
                print(f"    ... and {len(hits) - 12} more")
            print()

    substance = sum(len(results.get(k, [])) for k in
                    ["trailing-participle", "manufactured-significance", "vague-attribution",
                     "puffery", "copula-avoidance", "negative-parallelism", "formulaic-ending",
                     "didactic-disclaimer", "knowledge-gap-hedge"])
    vocab = len(results.get("ai-vocabulary", [])) + len(results.get("stiff-diction", []))

    print("summary")
    print(f"  substance-level flags   {substance:>4}   ({per1k(substance)} per 1k words)")
    print(f"  vocabulary flags        {vocab:>4}   ({per1k(vocab)} per 1k words)")
    print(f"  em dashes               {len(results.get('em-dash', [])):>4}   "
          f"({len(results.get('spaced-em-dash', []))} spaced)")
    print(f"  sentence length         mean {mean:.1f}, sd {sd:.1f}"
          f"{'   <- flat rhythm, vary it' if sd < 6 and len(sents) > 5 else ''}")
    if results.get("machine-artifact"):
        print("  !! generator artifacts present — source is unambiguous; verify all citations")
    print()
    print("Density across several categories is the signal. Isolated hits are usually fine.")
    print("Fix substance flags first — reworded hollow prose is still hollow.\n")


if __name__ == "__main__":
    main()
