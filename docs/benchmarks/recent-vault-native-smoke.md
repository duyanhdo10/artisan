# Recent-vault native restart smoke

- Date: 2026-09-02
- Build: debug Tauri/WebView2 binary from the current uncommitted Version 0.1 working tree
- Host: Windows x64
- Scope: native folder picker, settings persistence, restart restore, watcher reconciliation and vault preservation

## Isolation and method

The smoke built Astian with a temporary bundle identifier,
`app.astian.desktop.restoresmoke`. This gave the run a dedicated Local AppData
directory and avoided production Astian settings. A disposable workspace vault
used Unicode Vietnamese folder and note names.

Windows UI Automation opened the real Tauri window and invoked the Rust-owned
folder picker. The Windows 11 common folder dialog did not expose its primary
button through UI Automation, so the harness invoked only that dialog's native
`IDOK` control after entering the exact fixture path. The application was then
closed through its native window close path and launched a second time without
supplying a vault path.

The harness checked settings JSON directly only from the isolated app-data
fixture. It did not print the stored absolute path. It hashed the original
Markdown before and after both launches, created a second Markdown file after
restart to exercise the restored watcher, and scanned the vault for Astian
artifacts.

## Result

The final smoke passed every assertion:

```json
{"firstSelection":true,"settingsSchema":1,"markdownPreserved":true,"restartAutoRestore":true,"watcherAfterRestore":true,"vaultArtifacts":0}
```

- The first launch selected and opened the Unicode vault through the native
  folder dialog.
- `<isolated-app-data>/settings.json` used schema version 1 and contained one
  recent vault entry.
- The settings file was outside the vault, and the original Markdown hash did
  not change.
- The second launch reached the opened-vault UI without another dialog and
  showed the original Unicode note.
- After restart, an externally created Unicode Markdown file appeared through
  watcher reconciliation.
- Native clean close succeeded on both launches.
- No `.astian-*` or settings artifact appeared in the vault.

After the run, the harness verified that no `astian.exe` process remained and
removed both the disposable vault and the exact isolated app-data directory.

## Disposition

Recent-vault persistence and `restore_last_vault` pass the Version 0.1 native
restart smoke. A recent-vault picker, unavailable-vault management and active
tab/session restore remain later Version 0.1 work.
