# ADR 0011: Persist active-note session state outside vaults

- Status: Accepted for Version 0.1
- Date: 2026-09-02

## Context

Astian should restore the active note when a vault is reopened, but Markdown is
the source of truth and unsaved content already belongs to the recovery-draft
workflow. Session persistence must not create files in a vault, leak absolute
paths through frontend IPC, or make a stale tab overwrite external changes.

## Options considered

1. Store the active note in browser local storage.
2. Add active-note metadata to each vault.
3. Store a versioned per-vault session document under Tauri app-local-data.

## Decision

Astian uses option 3:

- `<app-local-data>/session.json` has `schema_version: 1` and at most 10
  per-vault entries.
- A vault is identified by SHA-256 of its canonical absolute path. The document
  stores only that opaque identity and the active relative Markdown path; it
  never stores note content, base hashes or dirty editor state.
- Rust validates every stored relative path with the normal vault path rules.
  Restore reopens the latest bytes through the existing read primitive. A
  missing note returns no restored tab and cannot recreate or overwrite it.
- If an available recovery draft exists for the remembered note, React leaves
  the tab closed so the user must explicitly restore or discard the draft.
- Frontend commands contain only a relative path or no path. Absolute vault
  locations remain behind the Rust boundary.
- Session writes use a sibling temporary file, flush, `sync_all`, no-clobber
  create or `ReplaceFileW`, expected-bytes preflight and read-back verification.
  Corrupt/unsupported state is preserved and reported rather than reset.
- Stable failures use `session_corrupt`, `session_unsupported`,
  `session_read_failed`, `session_prepare_failed`, `session_write_failed`,
  `session_changed`, `session_locked`, `session_replace_failed` and
  `session_verification_failed`; inability to resolve app-local-data uses
  `session_location_unavailable`.

## Consequences

- Closing/reopening or switching recent vaults can restore a clean active note
  without writing metadata into Markdown or the vault.
- Relative note names are visible in Local AppData. This is local application
  state under the OS user boundary and must be covered by privacy documentation.
- Session restore never restores selection, scroll, undo history or unsaved
  content in this first slice. Multiple-tab ordering can extend the versioned
  entry shape in a future schema migration.
- A moved/deleted remembered note opens no tab. Watcher/index reconciliation
  remains responsible for showing current vault contents.
