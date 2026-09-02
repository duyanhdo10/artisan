# ADR 0012: Preflight vault mutations and rename by source handle

- Status: Accepted for Version 0.1
- Date: 2026-09-02

## Context

Astian Version 0.1 must create folders and rename or move notes and folders
without overwriting another entry, following a reparse point outside the vault,
or acting on a different source after an external filesystem race. Later link
updates turn one visible rename into a multi-file transaction, but the Version
0.1 primitive still needs an identity and recovery model that can be extended
without changing its safety boundary.

Path-only preflight is insufficient. Another process can replace a source path
after Astian validates it but before the rename call. Case-only renames also
need an intermediate name on Windows and can be interrupted between the two
steps.

## Sources

- [Microsoft: CreateFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
- [Microsoft: SetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle)
- [Microsoft: FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info)
- [Microsoft: Reparse point operations](https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-point-operations)

## Options considered

1. Validate paths, then call `std::fs::rename` directly.
2. Copy to the destination, verify, then delete the source.
3. Preflight under the vault write coordinator, bind the source identity to an
   open Windows handle, and rename that handle with no-clobber semantics.

## Decision

Astian uses option 3 on Windows. Other platforms may keep a small path-based
implementation for tests and future ports, but Windows is the supported 1.0
platform and defines the safety contract.

### Command boundary

- Frontend commands send only a source relative path, a destination-parent
  relative path, one requested name segment and the expected source kind.
- A Markdown note rename/move also sends its last-read content hash. Rust reads
  and hashes the source again during preflight; mismatch returns
  `external_change_conflict` before any rename.
- The frontend must flush a dirty source note before calling the command. Rust
  does not trust that UI state and still requires the expected hash.
- Version 0.1 renames one filesystem entry and does not rewrite inbound links.
  Safe inbound-link rewriting remains Version 0.4 work and must extend the
  transaction described below.

### Preflight under the write coordinator

Rust performs all of these checks while holding the vault write coordinator:

1. Revalidate every relative input and normalize only the newly requested name
   according to ADR 0010.
2. Canonicalize the current vault root and destination parent. Reject a source,
   destination parent or traversed ancestor that is a reparse point or resolves
   outside the canonical vault.
3. Enumerate the latest destination namespace and fail closed on case or NFC
   collisions. Ordinary operations never replace an existing entry.
4. Open the source with `DELETE` access, sharing compatible with normal editor
   reads, `FILE_FLAG_BACKUP_SEMANTICS` for directories and
   `FILE_FLAG_OPEN_REPARSE_POINT`. Reject reparse points and a kind mismatch.
5. Bind identity and subsequent rename to that open handle. The destination is
   passed as an absolute path built only from the canonical parent and validated
   segment; it is never accepted from the frontend.

### Rename and move primitive

- Use `SetFileInformationByHandle` with `FileRenameInfoEx`/`FILE_RENAME_INFO`.
  Replacement flags remain clear so an existing destination causes failure.
- An ordinary rename/move is one handle-based operation followed by verification
  that the source path is absent and the destination is the same opened entry.
- A case-only rename uses a unique `.astian-rename-*` sibling intermediate and
  two no-clobber handle renames. The same rule applies on a filesystem whose
  case behavior makes a direct rename ambiguous.
- Astian records expected watcher effects only after filesystem verification.
  A watcher or session bookkeeping failure cannot turn a verified rename into a
  claim that the old source still exists.

### Transaction journal and recovery

- Before a two-step case-only rename, Astian durably writes a versioned journal
  under app-local data, keyed by the opaque vault identity. No journal, backup,
  cache or lock is stored in the vault.
- The journal records relative source, intermediate and destination paths,
  source kind and identity, but never note content.
- Startup reconciliation inspects the exact recorded paths without following
  reparse points. If identity proves which step completed, it finishes or rolls
  back deterministically. If identity is missing or ambiguous, Astian preserves
  all entries and asks for user recovery instead of guessing.
- Version 0.4 multi-file link rewriting must add expected hashes, prepared
  replacement artifacts and completion state to this journal. Every Markdown
  file is reparsed immediately before preparation; rollback modifies a file
  only while its hash still matches bytes written by that transaction.

### Create-folder primitive

Folder creation is a single-entry operation and needs no transaction journal:

1. Validate/NFC-normalize one folder segment and canonicalize the parent.
2. Enumerate the latest namespace under the write coordinator.
3. Call create-directory with no-clobber semantics.
4. Verify the result is a real directory, not a reparse point, and return its
   actual relative path. A concurrent target is preserved and reported as
   `name_collision`.

## Stable error categories

The primitives use structured categories, including:

- `invalid_folder_name`, `reserved_folder_name`, `folder_name_too_long`
- `folder_parent_unavailable`, `folder_create_failed`
- `folder_create_verification_failed`
- `invalid_mutation_path`, `mutation_source_unavailable`
- `mutation_kind_mismatch`, `mutation_reparse_forbidden`
- `external_change_conflict`, `name_collision`
- `mutation_prepare_failed`, `mutation_apply_failed`
- `mutation_verification_failed`, `mutation_recovery_required`

Messages stay redacted and never include absolute paths or note content.

## Consequences

- Rename/move implementation requires a small Windows-specific handle wrapper,
  explicit verification and failure-injection tests rather than a portable
  one-line rename.
- Single-entry ordinary moves do not need content backups and remain atomic on
  the current vault volume. Case-only and future multi-file operations pay the
  cost of a durable app-local journal.
- Version 0.1 can ship rename/move without guessing about links; Version 0.4 can
  add proven link rewrites without weakening source identity or rollback rules.
- Tests must cover traversal, absolute and UNC paths, source/destination reparse
  points, kind mismatch, content-hash conflict, permission/lock failure,
  case/NFC collision, case-only rename interruption, verification failure and
  ambiguous recovery.
