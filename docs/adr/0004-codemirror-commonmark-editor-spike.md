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
