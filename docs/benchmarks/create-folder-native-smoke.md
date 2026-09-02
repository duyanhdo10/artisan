# Create-folder native smoke

- Date: 2026-09-02
- Build: debug Tauri/WebView2 binary from the create-folder UI working tree
- Host: Windows x64
- Scope: restored vault, Unicode folder creation, collision safety, external
  folder reconciliation, restart persistence and vault-artifact cleanup

## Isolation and method

The smoke built Astian with the temporary bundle identifier
`app.astian.desktop.createfoldersmoke`. The committed harness at
`scripts/windows/create-folder-native-smoke.ps1` used a disposable vault under
the Windows temporary directory and the matching isolated Local AppData
profile. It refuses a pre-existing profile by default and validates exact
cleanup targets before recursively removing them.

Windows UI Automation drove the real Tauri/WebView2 window. The harness seeded
only version-1 recent-vault settings outside the vault, waited for the
accessible `Create folder` control, entered the Unicode name `Kế hoạch`, and
activated the form's `Create` button.

It then requested `kế hoạch`, which differs only by case, required the typed
collision message, and verified that exactly one matching folder remained.
Next, the harness created `External/Empty` outside Astian and required both
folders to appear through watcher reconciliation. Finally, it closed the app
through the native window path, restarted it, and required all folders to be
visible without another vault selection.

The harness terminates only the isolated test process tree, checks the vault for
`.astian-*`, `settings.json` and `session.json`, and cleans the exact disposable
vault and app-data profile.

## Result

Every final assertion passed:

```json
{"restoredVault":true,"unicodeCreate":true,"collisionPreserved":true,"externalFolderObserved":true,"restartVisible":true,"vaultArtifacts":0}
```

- The empty vault restored from app-local settings.
- `Kế hoạch` was created as a real empty directory and appeared in the tree.
- The case-equivalent request produced a visible error without replacing or
  duplicating the destination.
- External empty nested folders appeared after watcher reconciliation.
- Restart preserved the tree because folders are scanned from the vault, not
  stored as Astian metadata.
- No persistent or temporary Astian artifact appeared in the vault.

## Disposition

The Version 0.1 root-level create-folder flow passes its native Windows smoke on
the tested local NTFS environment. Creating inside a selected nested folder,
junction/reparse fixtures, case-sensitive directories, permission/disk-full
injection and long-path/network/removable matrices remain separate coverage.
