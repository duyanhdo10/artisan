# ADR 0007: Use bundled SQLite FTS5 through rusqlite

- Status: Accepted for the Windows technical spike
- Date: 2026-09-01

## Context

Astian needs a local, disposable full-text index for filename, title, and note
content. The Windows build must provide the same FTS5 behavior regardless of the
SQLite version installed on the host. Vietnamese queries need measured behavior
with and without diacritics, and one changed note must not require a full-vault
rebuild.

## Options considered

1. Use the SQLite library installed on the host.
2. Bundle SQLite and access it through `rusqlite`.
3. Use a separate search engine such as Tantivy.

## Decision

Astian uses `rusqlite` 0.40.2 with its `bundled` feature for the spike.

- The index database belongs under app-local data, partitioned by vault
  identity. It must never be created inside the vault and is never a source of
  truth.
- FTS5 indexes relative path, display title, and searchable content. A normal
  `notes` table stores identity and content hash for reconciliation.
- `unicode61 remove_diacritics 2` is used so Vietnamese Latin characters are
  case-folded and diacritic-insensitive. Prefix indexes of length 2, 3, and 4
  support quick-open-style partial terms.
- Full rebuild and each single-note upsert/delete use explicit SQLite
  transactions. Search query text is converted into quoted prefix terms instead
  of being accepted as raw FTS syntax.
- Ranking weights title above path and path above content. Snippets are produced
  locally by FTS5 and must be rendered as text unless a later sanitizer contract
  explicitly permits markup.
- Schema version mismatch is rejected so the caller can discard and rebuild the
  cache from current Markdown rather than attempt an unsafe migration.

## Consequences

- Windows builds are larger, but FTS5 availability and SQLite behavior are
  reproducible and do not depend on a machine-wide DLL.
- Diacritic-insensitive search improves Vietnamese recall but cannot distinguish
  words that differ only by tone marks. A future product search UI may offer an
  exact mode if user testing requires it.
- The spike proves the core transaction and query behavior. Background indexing,
  watcher integration, corruption replacement, cancellation, progress UI, and
  app-local database lifecycle remain Version 0.4 work rather than being implied
  by this benchmark module.
- The reference 10,000-note run measured warm search p95 at 33.301 ms and
  incremental update p95 at 51.489 ms. The approach is `go with guardrails`;
  detailed methodology and limitations are recorded in
  `docs/benchmarks/fts5-10000-notes.md`.
