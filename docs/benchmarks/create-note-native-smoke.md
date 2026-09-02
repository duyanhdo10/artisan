# Create-note native smoke

- Date: 2026-09-02
- Build: debug Tauri/WebView2 binary from commit `81f7767`
- Host: Windows x64
- Scope: native vault selection, Unicode note creation, collision safety,
  restart visibility and vault-artifact cleanup

## Isolation and method

The smoke built Astian with the temporary bundle identifier
`app.astian.desktop.createnotesmoke`. This isolated the run from normal Astian
settings in a dedicated Local AppData directory. The vault was a disposable
Unicode-named directory under the Windows temporary directory.

Windows UI Automation drove the real Tauri/WebView2 window. It invoked the
Rust-owned folder picker, entered the exact fixture path and activated the
native dialog's `IDOK` control. It then used the accessible `Create note`
button, `New note name` input and `Create` action to request `Kế hoạch`.

After verifying the created file directly, the harness requested
`kế hoạch.MD`. This differs from the stored name only by ASCII case and the
extension's case, so it exercises the conservative collision key without
depending on keyboard-layout behavior. The harness required the typed
collision message to appear and verified the original file remained the only
Markdown file with unchanged empty bytes.

Finally, the app closed through its native window path and restarted without
another vault selection. The harness checked the note was visible, scanned the
vault for `.astian-*` and `settings.json`, then closed the app and removed only
the exact disposable vault and isolated app-data directory.

## Result

Every final assertion passed:

```json
{"firstSelection":true,"unicodeCreate":true,"collisionPreserved":true,"restartVisible":true,"vaultArtifacts":0}
```

- The native dialog opened the Unicode vault.
- `Kế hoạch.md` was created as an empty UTF-8 Markdown file.
- The case/extension-equivalent second request produced the visible collision
  error and did not create, replace or modify another file.
- Restart restored the vault and showed the created note.
- No Astian settings, temp or recovery artifact appeared in the vault.

After the run, no `astian.exe` process or isolated smoke app-data directory
remained.

## Disposition

The Version 0.1 root-level create-note flow passes its native Windows smoke for
the tested local NTFS environment. Nested-folder creation, permission/disk-full
injection, case-sensitive directories, network/removable storage and long-path
matrices remain separate follow-up coverage.
