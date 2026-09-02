# Active-note session native smoke

- Date: 2026-09-02
- Build: debug Tauri/WebView2 binary from the Version 0.1 active-session working tree
- Host: Windows x64
- Scope: durable active-note state, restart restore, stale-note behavior,
  content privacy and vault preservation

## Isolation and method

The smoke used the temporary identifier
`app.astian.desktop.activesessionsmoke`, a dedicated Local AppData directory and
a disposable Unicode-named vault. The harness seeded only the isolated recent
vault setting so the real Tauri app could restore the vault without a dialog.

Windows UI Automation opened the note through its file-list button. The harness
waited for `session.json`, verified it contained the relative note name and did
not contain a unique marker from the Markdown body, then closed the app through
its native window path.

On the second launch, the harness required the exact `Kế hoạch.md` active tab
to appear without another click. It closed the app, deleted that Markdown file
as an external actor, and launched a third time. The third launch had to show
the `Welcome` tab while the deleted file remained absent.

The harness scanned the vault for `.astian-*`, `settings.json` and
`session.json`. It verified no test `astian.exe`, app-data or temporary fixture
remained after cleanup.

## Result

Every final assertion passed:

```json
{"openedAndRemembered":true,"sessionHasNoContent":true,"restartRestoredTab":true,"staleNoteNotRestored":true,"staleNoteNotRecreated":true,"vaultArtifacts":0}
```

- Opening the note created durable, versioned session state outside the vault.
- Session state contained the active relative path but no note-body marker.
- Restart restored the clean active tab and latest disk bytes.
- A remembered note deleted externally was neither opened nor recreated.
- No persistent or temporary Astian artifact appeared in the vault.

## Disposition

The single-active-note Version 0.1 session slice passes its native Windows
smoke. Multiple-tab ordering, selection/scroll state, session contention across
multiple app instances and explicit corrupt-session management remain future
coverage. Recovery drafts remain the only mechanism for unsaved content.
