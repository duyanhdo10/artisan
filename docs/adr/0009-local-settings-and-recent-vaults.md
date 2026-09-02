# ADR 0009: Store versioned settings outside the vault

- Status: Accepted for Version 0.1 foundation
- Date: 2026-09-02

## Context

Astian must reopen the most recent vault after restart and later support a
recent-vault picker. The stored location is application state, not Markdown,
and may reveal local folder names. A partial or incompatible settings write
must not prevent the user from choosing a vault manually or place artifacts in
the vault.

## Options considered

1. Store settings in each vault.
2. Store the last vault in frontend browser storage.
3. Store a versioned JSON document below Tauri's app-local-data directory and
   keep absolute paths behind the Rust IPC boundary.

## Decision

Astian uses option 3:

- `<app-local-data>/settings.json` has an explicit `schema_version` and a
  most-recent-first list capped at 10 canonical vault paths.
- Rust owns reading, validation and writing. The frontend can request
  `restore_last_vault`, but the response contains only the existing
  relative-path `VaultSummary`; absolute vault paths do not cross IPC.
- Paths must be absolute and representable without lossy conversion. A stored
  path is canonicalized again before use because persisted paths are untrusted
  input on the next launch.
- Writes use a sibling temporary file, flush, `sync_all`, atomic create or
  `ReplaceFileW`, and read-back verification. Updating an existing file checks
  its previously read bytes before every replace attempt; a changed revision is
  preserved and reported instead of overwritten.
- Corrupt or unsupported settings are preserved and reported with stable
  `settings_corrupt` or `settings_unsupported` codes. Missing settings mean no
  recent vault and are not an error. An unavailable recent vault uses
  `recent_vault_unavailable` and leaves manual vault selection available.
- Other settings failures use stable `settings_*` error codes with redacted
  messages. Paths are not logged or embedded in errors.

## Consequences

- The vault remains free of Astian application data.
- Local AppData contains plaintext absolute vault paths. Astian relies on the
  OS user-account boundary for Version 1.0 and must document this privacy
  behavior before release.
- Moving or deleting a vault can make the most recent entry unavailable; Astian
  does not guess a replacement path.
- The byte precondition narrows multi-process races but Windows does not offer a
  content compare-and-swap primitive. Single-instance behavior remains required
  in Version 0.1.
- Future incompatible settings changes must increment the schema and provide an
  explicit migration or safe fallback; they must not reinterpret unknown data
  silently.
