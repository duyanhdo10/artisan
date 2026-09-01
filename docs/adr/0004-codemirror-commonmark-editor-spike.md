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
- Live Preview has a provisional 512 KiB UTF-8 soft limit. Larger notes remain
  fully editable and saveable in Source mode; the mode change is presentation
  only and does not replace the CodeMirror document or history.

## Consequences

- CodeMirror and Lezer become frontend dependencies and increase the frontend
  bundle; bundle and typing latency must be measured before the spike exits.
- CommonMark is the spike baseline, not yet the final Astian Markdown dialect.
  GFM features, wiki links, frontmatter, tables, footnotes, and math require an
  explicit parser-extension decision before Version 0.2.
- The initial demo decorates only ATX headings and strong emphasis. Other
  Markdown remains editable and visible as source.
- Automated tests cover source preservation, selection-driven syntax reveal,
  Unicode edits, undo/redo, and mode switching. A visible Windows/Tauri smoke
  now also covers real Vietnamese Telex composition, selection reveal,
  Source/Live mode switching, typing, undo/redo, safe-save, large-note fallback,
  and dirty/external conflict handling.

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
so it must not block interactive startup. The provisional 512 KiB soft limit is
intentionally below the 728 KiB fixture where initial parse p99 reached 261.50
ms. It must be recalibrated with visible DOM/cold-start measurements rather than
treated as a permanent format limit. Viewport/incremental decoration work as
syntax coverage grows remains required before production approval.

The visible Windows run on 2026-09-01 measured a 22-character WebView2/UI
Automation round trip at p50 27.74 ms, p95 49.87 ms, and max 52.36 ms. It also
observed real `Tieengs Vieetj` → `Tiếng Việt` composition and a 543,172-byte note
falling back to Source mode in 197.71 ms. These numbers include automation
overhead and do not replace browser event-to-paint profiling. Full method,
conflict evidence, and remaining gaps are recorded in
`docs/benchmarks/windows-editor-native-smoke.md`.

A follow-up production build defers the CodeMirror component until a note is
opened. This reduced the initial JavaScript chunk from 543.62 kB (174.21 kB
gzip) to 228.17 kB (70.34 kB gzip), with the 315.79 kB editor chunk loaded on
demand. The release Tauri/NSIS build and a packaged-editor native smoke both
passed, so the split does not depend on a development server.
