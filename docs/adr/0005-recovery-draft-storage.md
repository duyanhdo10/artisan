# ADR 0005: Recovery drafts live outside the vault

- Status: Accepted for Version 0.1 foundation
- Date: 2026-08-31

## Context

Astian must survive a crash while a note has unsaved edits, without changing
the user's Markdown or creating permanent application data in the vault. A
recovery copy is sensitive plaintext and must not be indexed, logged, synced by
Astian, or confused with the saved note. Cleanup must never happen before the
corresponding save is verified.

## Options considered

1. Store hidden recovery files beside each Markdown note.
2. Store drafts in a single global temporary directory.
3. Store versioned drafts below Tauri's app-local-data directory, partitioned
   by a hash of the canonical vault identity.
4. Keep dirty content only in memory and rely on normal autosave.

## Decision

Astian uses option 3. Tauri's `app_local_data_dir()` is the root because it
resolves to the bundle identifier below Windows Local AppData. The logical
layout is:

```text
<app-local-data>/
└── vaults/
    └── <vault-id>/
        └── recovery/
            └── <note-id>.json
```

- `vault-id` is SHA-256 over the canonical vault identity bytes. Path identity
  normalization must use the same canonicalization contract as vault opening.
- `note-id` is SHA-256 over the validated, slash-normalized relative note path.
  The filename does not expose a note name.
- Each JSON draft has `schema_version`, `vault_id`, `relative_path`,
  `base_hash`, `content_hash`, `editor_revision`, `updated_at_unix_ms`, and the
  exact normalized editor `content`.
- Recovery JSON is written through a unique sibling temporary file, flushed,
  synced, and atomically replaced. Operations are serialized with the vault
  write coordinator.
- A dirty editor revision is not considered recovery-protected until the Rust
  command confirms the durable draft write.
- Creating a draft uses no-clobber semantics. Replacing one requires the caller
  to provide its last confirmed `content_hash`; a missing or mismatched hash
  refuses the write so an unreviewed draft from an earlier session cannot be
  overwritten by a new editing session.
- A draft is removed only after the target note save has been read back and
  hash-verified, or after an explicit user action discards that draft.
- Cleanup failure does not turn a verified note save into a failed save. The
  stale draft remains and is reconciled on next startup by comparing hashes.
- Corrupt, unsupported-schema, identity-mismatched, or hash-mismatched recovery
  data is never applied automatically or deleted silently. Astian reports it
  as unavailable recovery data and leaves it for explicit cleanup/export.
- Recovery listing is a typed `available`/`unavailable` union. Unavailable
  entries expose only an opaque hashed recovery ID, artifact hash, and stable
  reason; raw bytes, note content, and untrusted paths do not cross into the UI.
- Export of unavailable data uses a native save dialog and durable sibling-temp
  plus create-no-clobber semantics. Explicit delete requires the artifact hash
  returned by listing, revalidates that the artifact is still unavailable, and
  uses a two-step confirmation in the UI.
- Recovery content is not returned in a list call. The UI first receives typed
  metadata and requests one selected draft by identity when restoring.
- Recovery content and relative paths must not appear in logs or analytics.

## Startup and conflict behavior

When a vault opens, Astian validates recovery metadata against that vault:

- If draft `content_hash` equals the current disk content hash, the draft is
  stale and may be removed after reconciliation.
- If disk still matches `base_hash` but content differs, offer restore into a
  dirty editor buffer; do not write the note automatically.
- If disk differs from both hashes, open conflict recovery with `Save As Copy`
  as the preferred non-destructive action.
- A missing/renamed target remains recoverable as content that can be saved as
  a new copy; Astian does not guess a rename from path similarity.
- Corrupt and unsupported entries do not block valid drafts from being listed.
  They remain in app-local-data until the user exports or explicitly deletes
  the exact artifact revision shown by the recovery manager.

## Consequences

- The vault remains ordinary user-owned Markdown with no Astian recovery files.
- A second plaintext copy can exist under Local AppData. Version 1.0 relies on
  OS account permissions and documents this; application-managed encryption is
  deferred unless a separate threat-model ADR approves key storage and recovery.
- Moving a vault creates a new vault identity. Old drafts remain discoverable
  through recovery management instead of being silently reassigned.
- Durable-write, corrupt-draft, cleanup-failure, traversal, Unicode, and crash
  ordering tests are required before autosave may rely on this mechanism.
