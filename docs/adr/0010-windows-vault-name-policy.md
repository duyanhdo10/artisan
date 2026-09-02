# ADR 0010: Use a conservative Windows vault-name policy

- Status: Accepted for Version 0.1
- Date: 2026-09-02

## Context

Astian needs deterministic create, rename and move behavior on Windows without
silently changing a user's requested path or producing names that Explorer,
Git or another Markdown editor cannot address reliably. NTFS normally compares
names case-insensitively, but Windows can enable case sensitivity per directory.
NTFS also preserves distinct Unicode normalization forms even when they look
identical.

The policy must distinguish files Astian creates from pre-existing vault data.
Existing Markdown remains user-owned source data and must not be renamed or
normalized automatically.

## Sources

- [Microsoft: Naming Files, Paths, and Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file)
- [Microsoft: Maximum Path Length Limitation](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)
- [Microsoft: CompareStringOrdinal](https://learn.microsoft.com/en-us/windows/win32/api/stringapiset/nf-stringapiset-comparestringordinal)
- [Microsoft: Using Unicode Normalization to Represent Strings](https://learn.microsoft.com/en-us/windows/win32/intl/using-unicode-normalization-to-represent-strings)
- [Microsoft: Case Sensitivity](https://learn.microsoft.com/en-us/windows/wsl/case-sensitivity)

## Options considered

1. Pass user input directly to NTFS and report only OS errors.
2. Lowercase or aggressively sanitize every name.
3. Validate each requested segment, normalize newly created names to NFC, and
   reject ambiguous/colliding names without changing existing external names.

## Decision

Astian uses option 3 for every UI-originated create, rename and move.

### Input and canonical output

- Frontend validation is advisory. Rust repeats all validation immediately
  before filesystem I/O.
- Commands accept a parent relative path and one user-entered segment, never an
  absolute destination or a combined path supplied by the frontend.
- Astian rejects empty names, `.`/`..`, path separators, NUL, C0 controls and
  Win32 reserved characters `< > : " / \ | ? *`.
- Leading or trailing Unicode whitespace is rejected rather than trimmed.
  Trailing periods are rejected. A leading period remains legal.
- Device basenames are rejected case-insensitively even with extensions:
  `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, and the Windows
  superscript variants `COM¹`–`COM³` and `LPT¹`–`LPT³`.
- Names beginning with the case-insensitive internal prefix `.astian-` are
  reserved for Astian filesystem primitives and cannot be created through UI.
- New note names are normalized to Unicode NFC. Astian appends `.md` when it is
  absent and canonicalizes an explicitly supplied `.MD` suffix to `.md`. An
  empty note stem is invalid. The returned relative path is the actual stored
  name and becomes the frontend identity.
- One path segment may contain at most 255 UTF-16 code units including the
  extension. The complete target and every sibling temporary/recovery name must
  remain below the legacy `MAX_PATH` boundary in Version 0.1. This deliberately
  conservative restriction remains until the application manifest, Explorer,
  Git and failure-injection matrix prove long-path support end to end.

### Collision semantics

- Before create/rename/move, Rust enumerates the latest parent directory under
  the vault write coordinator; it does not rely on a stale UI file list.
- A collision key is NFC-normalized and compared with Windows ordinal
  case-insensitive semantics. Canonically equivalent names and names differing
  only by case therefore collide even in a case-sensitive NTFS directory.
- Astian never chooses a numeric suffix or overwrites automatically. It returns
  a stable `name_collision` error and asks the user for another name.
- A case-only rename of the same entry is a separate explicit operation. It
  requires source identity/preflight and a two-step no-clobber transaction; it
  is not treated as ordinary create.
- If a directory already contains multiple entries that collapse to one
  collision key, Astian shows them but disables ambiguous create/rename/link
  operations in that namespace. It never guesses which entry is intended.

### Existing external names

- Scan/open preserve exact discovered names, casing and normalization.
- A safe existing Markdown file that violates the UI creation policy remains
  readable and editable. Astian does not rename it, rewrite links to it, or
  hide it merely because its name is non-preferred.
- Operations that would need to create another invalid or ambiguous name fail
  closed with a typed error. External case/normalization collisions are surfaced
  as a vault issue for later management.

## Create-note primitive

Creating a note is distinct from replacing an existing note:

1. Validate and NFC-normalize the requested segment and validate the canonical
   parent inside the current vault.
2. Enumerate the parent and perform collision comparison immediately before
   installation.
3. Write an empty UTF-8/LF Markdown payload to a unique sibling temporary file,
   flush it and call `sync_all`.
4. Install with create/no-clobber semantics. An existing or concurrently
   created target is preserved.
5. Read back and hash the created file before returning `OpenedNote`.
6. Register the verified hash with watcher reconciliation. Failure after a
   verified create must not report that the file does not exist.

## Stable error categories

The first implementation uses redacted messages with stable categories:

- `invalid_note_name`
- `reserved_note_name`
- `note_name_too_long`
- `note_parent_unavailable`
- `name_collision`
- `note_create_prepare_failed`
- `note_create_write_failed`
- `note_create_install_failed`
- `note_create_verification_failed`

## Consequences

- UI-created paths remain predictable across default NTFS, case-sensitive NTFS,
  Explorer and Git, at the cost of refusing some names the underlying volume
  could technically store.
- NFC changes only newly requested names and is visible in the command result;
  existing user filenames are never normalized in place.
- Version 0.1 does not claim general long-path, network-share, removable-volume
  or OneDrive correctness. Those environments remain best-effort until their
  dedicated matrix passes.
- Tests must cover invalid characters, reserved device names (including
  extensions and superscript digits), Unicode NFC/NFD collision, case-only
  collision, existing target, install race, Unicode success, cleanup and
  verification failure.
