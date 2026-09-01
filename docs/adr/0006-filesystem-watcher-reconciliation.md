# ADR 0006: Watcher events trigger hash-based reconciliation

- Status: Accepted for the Windows technical spike
- Date: 2026-09-01

## Context

Windows filesystem notifications can arrive as bursts, duplicate operations,
or incomplete rename information. Astian must notice changes made by editors or
Git without treating a notification as proof of final disk state, creating a
save loop, or overwriting a dirty editor buffer.

## Options considered

1. Apply each native event directly to the file tree and active editor.
2. Poll and hash the whole vault on a fixed interval.
3. Use native recursive notifications as hints, debounce them, then rescan and
   compare content-hash snapshots.

## Decision

Astian uses option 3 with `notify` 8.2.0 and its recommended Windows watcher.

- Raw activity is coalesced for 150 ms. The reconciler then validates Markdown
  paths through the existing vault boundary and computes the final hash
  snapshot; raw event paths and event kind are not trusted.
- The technical spike rescans all Markdown notes. This is intentionally simple
  evidence code and must become an incremental candidate scan before large
  vault production approval.
- Reconciliation shares the vault write coordinator. A verified Astian write
  registers `(relative_path, expected_hash)` before the coordinator is released,
  so the following scan can classify it as `astian`; a missing or mismatched
  hash is always `external`.
- Each opened vault has an opaque numeric session and each emitted event has a
  monotonic revision. The frontend ignores stale sessions/revisions and
  serializes event handling.
- A unique removed/created pair with the same content hash is reported as a
  rename, including Unicode and case-only rename. Duplicate hash candidates are
  never guessed; they remain explicit delete/create changes.
- A clean active note reloads or follows an external rename. An external change
  to a dirty/saving note enters conflict and stops autosave. A clean external
  delete closes the editor without writing.
- Backend errors or a locked/unreadable note keep the prior snapshot, retry with
  bounded delays, and emit `rescan_required` if still unresolved. Window focus
  requests another reconciliation.
- Events contain relative paths, hashes, classification, and the refreshed note
  list. They never contain absolute vault paths or note content.

## Consequences

- Event loss, duplication, and burst ordering do not directly corrupt frontend
  state because disk rescan is the source of truth.
- Full hashing while holding the write coordinator is conservative and can
  delay save on a large vault. The SQLite/index milestone must introduce
  incremental candidate selection, overflow recovery, and measured 10,000-note
  performance before this strategy is production-ready.
- Content-equal rename pairing is safe only when unique. Ambiguous rename UX is
  deferred rather than inferred from filename similarity.
