# ADR 0003: Replace existing notes with `ReplaceFileW`

- Status: Accepted for the Windows technical spike
- Date: 2026-08-31

## Context

Writing directly into a Markdown file can leave it truncated or partially written if Astian, Windows, or the storage device fails mid-write. A plain delete-then-rename introduces a window where the note does not exist and can lose Windows metadata. Astian also needs optimistic concurrency so an editor or Git operation cannot be overwritten silently.

## Options considered

1. Truncate and rewrite the target in place.
2. Write a sibling temporary file and call Rust `std::fs::rename` over the target.
3. Write and flush a sibling temporary file, then call Windows `ReplaceFileW` with a recovery backup.

## Decision

For the Windows spike, Astian uses option 3 for existing notes:

1. Canonicalize and validate the target inside the active vault.
2. Read bytes and compare SHA-256 with the caller's `expectedHash`.
3. Return unchanged without writing if normalized editor content has not changed.
4. Preserve the existing UTF-8 BOM and uniform LF/CRLF convention while encoding edited content. Mixed line endings remain read-only in this spike.
5. Create an unpredictable sibling temporary file, write all bytes, flush, and call `sync_all`.
6. Re-read and compare the target hash immediately before every replace attempt.
7. Call `ReplaceFileW` with a unique sibling recovery path. Retry only bounded lock/access errors.
8. Read and hash the resulting target before reporting success, then remove the recovery file.

`ReplaceFileW` is called without flags that ignore ACL/metadata merge errors. If recovery cannot be proven after a platform error, Astian reports failure and retains the recovery artifact rather than guessing or deleting the last known-good bytes.

Safe creation is now a separate primitive for recovery `Save As Copy`:

1. The user chooses a Markdown filename, but Rust canonicalizes its existing
   parent and rejects destinations outside the active vault.
2. Normalized UTF-8/LF content is written to a unique sibling temporary file,
   flushed, and synced.
3. A hard link installs that inode at the destination with create/no-clobber
   semantics. An existing or concurrently created destination is never
   overwritten, even if the platform dialog offered overwrite confirmation.
4. Rust reads and verifies the created bytes before returning the opened copy.

## Consequences

- Existing-note save and no-clobber copy creation remain separate operations;
  ordinary new-note creation is still outside this spike.
- A very small check-to-replace race with unrelated processes still exists because Windows has no file-content compare-and-swap primitive. Rechecking before each attempt narrows the window, and watcher reconciliation remains required.
- A crash between replacement and cleanup can leave a non-Markdown `.astian-backup-*` recovery file beside the note. A later recovery journal/cleanup design must move durable recovery state outside the vault before Version 0.1 is complete.
- Network, removable, OneDrive, and unusual filesystems remain best-effort until failure-injection tests establish their behavior.
