# ADR 0001: Keep the desktop application in one repository

- Status: Accepted
- Date: 2026-08-31

## Context

Astian ships its React frontend, Rust/Tauri native layer, installer, and updater as one desktop application. Changes to the IPC boundary commonly require coordinated TypeScript and Rust updates.

## Decision

Keep frontend and backend in one Git repository. Frontend code lives in `src/` and native code lives in `src-tauri/`. They share one application version and release tag.

## Consequences

- IPC changes can be reviewed and tested atomically.
- One CI pipeline produces the complete Windows artifact.
- A separate repository will only be considered for a component with independent deployment, versioning, and consumers.
