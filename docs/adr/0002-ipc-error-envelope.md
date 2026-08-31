# ADR 0002: Use stable error codes across the Tauri IPC boundary

- Status: Accepted
- Date: 2026-08-31

## Context

The React frontend needs to distinguish conflicts, invalid paths, unavailable files, and internal failures. Human-readable error messages will evolve and may eventually be localized, so they cannot be a reliable programmatic contract.

## Decision

Every fallible Tauri command returns a serializable error object with a stable `code` and a redacted `message`. TypeScript normalizes unexpected transport failures into the same shape. UI behavior branches on `code`, while `message` is only presented to the user or used as diagnostic context.

## Consequences

- Rust and TypeScript command contracts must change together.
- New error categories require documented stable codes and tests.
- Internal causes may be retained inside Rust, but note content, paths, credentials, and other sensitive data must not cross into messages or logs without an explicit safe reason.
