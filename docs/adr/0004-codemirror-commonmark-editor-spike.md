# ADR 0004: CodeMirror 6 with a shared CommonMark syntax tree

- Status: Accepted for the Windows technical spike
- Date: 2026-08-31

## Context

Astian needs a Markdown editor that can offer Source and Live Preview modes
without parsing and serializing the user's document. Decorations must not break
selection, Vietnamese IME input, or undo/redo. The editor and future indexer
also need an explicit syntax baseline instead of unrelated regular-expression
parsers.

## Options considered

1. Keep a controlled React `textarea` and build preview in a separate renderer.
2. Use CodeMirror 6 with its Lezer Markdown syntax tree and state decorations.
3. Use a rich-text editor that imports and exports Markdown.

## Decision

For the technical spike, Astian uses CodeMirror 6 and the strict CommonMark
language as the source syntax tree. Live Preview is a presentation-only
extension:

- The editor document remains the exact Markdown string sent to Rust.
- Source mode removes preview decorations without replacing the document or
  resetting editor history.
- Heading line styles and strong-emphasis markers are derived from syntax-tree
  node ranges, not regular-expression matching.
- Syntax is revealed when the selection enters the owning Markdown node.
- React synchronizes an externally loaded document with a transaction excluded
  from undo history; ordinary edits stay in CodeMirror history.
- Raw HTML preview, inline active content, and parser-driven Markdown
  serialization are not part of this spike.

## Consequences

- CodeMirror and Lezer become frontend dependencies and increase the frontend
  bundle; bundle and typing latency must be measured before the spike exits.
- CommonMark is the spike baseline, not yet the final Astian Markdown dialect.
  GFM features, wiki links, frontmatter, tables, footnotes, and math require an
  explicit parser-extension decision before Version 0.2.
- The initial demo decorates only ATX headings and strong emphasis. Other
  Markdown remains editable and visible as source.
- Automated tests cover source preservation, selection-driven syntax reveal,
  Unicode edits, undo/redo, and mode switching. Real Vietnamese IME composition
  still requires a visible Windows smoke test.

## Spike evidence and disposition

Disposition: **Go with guardrails** for the CodeMirror 6/CommonMark editor
architecture. This does not yet approve every planned Live Preview element.

Reference run on 2026-08-31:

- Windows `10.0.26300.9278`, Intel Core i5-8300H, 16 GB RAM, Node `26.8.1`.
- Fixture: 5,000 syntax-dense blocks, 728 KiB Markdown, 25,000 preview
  decorations.
- Full parse to EOF plus all decorations: mean 147.78 ms; p99 261.50 ms.
- One-character edit at EOF plus full decoration rebuild: mean 18.81 ms; p99
  24.01 ms.
- Selection move plus full decoration rebuild: mean 13.56 ms; p99 19.36 ms.

The measured edit and selection work stays under the initial 50 ms long-task
budget on the reference machine. Initial full parsing can exceed that budget,
so it must not block interactive startup. Source-mode fallback, a measured note
soft limit, viewport/incremental decoration work as syntax coverage grows, and
a visible Vietnamese IME smoke test remain required before production approval.
