# ADR 0002: Span and offset representation

Status: Accepted

Date: 2026-08-16

## Context

The language requires exact raw-byte identity for syntax, hashes, stable IDs, diagnostics, and lock
records. Humans use one-based Unicode-scalar coordinates, while LSP clients use negotiated
zero-based UTF-8 or UTF-16 positions. Malformed UTF-8 and CRLF must still receive exact raw ranges.

## Decision

The canonical range is a checked zero-based, half-open `u64` byte range over the original source
bytes. Every range satisfies `start <= end <= source_length`. File size and every conversion from a
platform index are checked before entering this representation.

Tokens, CST nodes, semantic anchors, source maps, diagnostics, lock spans, hashes, and stable IDs use
raw byte ranges. A leading accepted BOM remains in CST preamble trivia, and all following byte
offsets retain their raw positions. Semantic processing ignores the BOM but does not renumber the
file. A file-level facet anchor remains the specified zero-length range at byte zero.

One shared line index is built by scanning raw bytes. LF ends a line. CRLF is one newline sequence
spanning two bytes. Bare CR does not end a line and receives its own lexical diagnostic. Invalid
UTF-8 does not prevent newline indexing.

For a diagnostic over malformed UTF-8, line calculation still uses that raw newline index. Column
calculation scans from the raw line start, advances once for each valid Unicode scalar, and advances
once for each invalid byte. These pseudo-scalar columns exist only on diagnostics for malformed
source. They never enter semantic anchors, stable IDs, or a generated lock.

Normative source coordinates are one-based lines and one-based Unicode-scalar columns with an
exclusive end position. Column one is the first scalar of a line. Combining scalars each advance
the column once. An astral scalar advances the normative column once.

Offsets at the CR and LF bytes of a CRLF sequence map to the preceding line's exclusive end
coordinate. The offset immediately after LF maps to column one of the next line. Reverse conversion
of that preceding line-end coordinate selects the boundary immediately before CR. Semantic anchors
may not begin or end inside a valid UTF-8 scalar or CRLF sequence.

At the protocol boundary, positions are zero-based. UTF-8 character offsets count bytes from the
line start, and UTF-16 offsets count code units. An astral scalar counts as four UTF-8 bytes and two
UTF-16 code units. The server selects UTF-8 when the client offers it, otherwise UTF-16. If the
client omits position encodings, the server uses UTF-16. UTF-32 is not selected.

Incoming LSP positions must identify a scalar boundary and must not exceed the line content. An
invalid incoming position returns an invalid-params response and never silently retargets an edit.
For malformed on-disk bytes that cannot exist in the client's text model, the published range is
clamped to the nearest representable enclosing client range, while diagnostic data retains the
exact raw byte range.

All conversions are fallible. No consumer performs unchecked `u64` to platform-index conversion,
locale-dependent column calculation, or independent line scanning.

## Consequences

Serialized IDs and lock spans are independent of editor encoding and host platform. One source map
can serve CLI, lock, and LSP consumers. Some raw diagnostic ranges cannot be represented exactly by
LSP and therefore require the explicit raw range in diagnostic data.

## Verification

Golden vectors cover empty input, EOF, one leading BOM, misplaced and repeated BOMs, LF, CRLF, bare
CR, ASCII, combining scalars, astral scalars, invalid UTF-8, and ranges on both sides of every line
boundary.

Property tests round-trip every valid scalar boundary through byte, normative, UTF-8, and UTF-16
coordinates. Tests also assert checked overflow, rejection of mid-scalar and mid-CRLF edits,
monotonic coordinate conversion, and stable source-span encoding.
