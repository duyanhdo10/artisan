# Astian

Astian is a local-first Markdown note-taking app for Windows.

Your notes stay as ordinary `.md` files inside a folder you own. Astian does not require an account, a proprietary cloud, or a database server, and your vault remains usable with any standard text editor.

> Astian is currently in pre-alpha development and technical validation. The application is not ready for everyday use yet.

## Planned experience

- Markdown Live Preview with a dedicated Source mode.
- Multiple tabs, autosave, file tree, Quick Open, and command palette.
- Wiki links, backlinks, tags, frontmatter, outline, and full-text search.
- Safe handling of changes made by Git or another editor.
- Portable attachments stored inside the vault.
- A focused Git workflow for status, commit, fast-forward pull, and push.
- Signed application updates through GitHub Releases.
- Optional AI features after the core note-taking experience is stable.

## Principles

- **Local-first:** core note-taking works without a network connection.
- **Markdown-native:** Markdown files are the source of truth.
- **User-owned data:** Astian does not hide notes inside a proprietary database.
- **Git-friendly:** indexes and application caches stay outside the vault.
- **Safety before convenience:** Astian must not silently overwrite external changes.
- **Private by default:** note content is not sent anywhere without an explicit action.

## Initial platform

- Windows 10/11 x64
- Dark theme
- React + TypeScript frontend
- Tauri + Rust native layer
- SQLite FTS5 rebuildable search index
