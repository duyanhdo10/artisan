# Recent-vault picker native smoke

- Date: 2026-09-02
- Build: debug Tauri/WebView2 binary from the Version 0.1 recent-vault picker working tree
- Host: Windows x64
- Scope: startup restore, recent-vault switching, unavailable entry management,
  durable settings update and vault preservation

## Isolation and method

The smoke used the temporary identifier
`app.astian.desktop.recentvaultsmoke`, a dedicated Local AppData directory and
two disposable local vaults. A third settings entry intentionally pointed to a
missing vault. The harness seeded only this isolated `settings.json`; no normal
Astian settings or user vault was accessed.

Windows UI Automation drove the real Tauri/WebView2 window. The harness
required the first vault's unique note to appear through startup restore,
located the unavailable entry through its accessible `Forget` action, opened
the second vault from the recent list and required its unique note to appear.
It then waited until `Forget Missing vault` completed and another recent action
was enabled before reading settings once.

The first diagnostic run exposed a real identity bug: a Win32 path stored as
`C:\...` and the `\\?\C:\...` path returned by canonicalization were compared as
raw strings, duplicating one vault when it was reopened. The implementation was
changed to deduplicate available entries by canonical filesystem identity, and
a Windows regression test now covers both path forms.

## Result

The final smoke passed every assertion:

```json
{"autoRestoreFirst":true,"openedSecond":true,"unavailableVisible":true,"forgotUnavailable":true,"settingsCount":2,"vaultArtifacts":0}
```

- Startup restored the first recent vault.
- Opening the second entry switched to the correct vault contents.
- The missing entry remained visible with an explicit Forget action.
- Forget removed only the missing entry; the durable settings list contained
  exactly the two available vaults with no duplicate path form.
- Neither vault contained settings or `.astian-*` artifacts.

After the run, no `astian.exe` process or isolated smoke app-data directory
remained. The exact disposable fixture was removed.

## Disposition

The Version 0.1 recent-vault picker passes its native Windows smoke for local
NTFS paths. Offline network shares, removable drives, same-display-name vaults
and rapid multi-window settings contention remain follow-up coverage; planned
single-instance behavior will further narrow settings races.
