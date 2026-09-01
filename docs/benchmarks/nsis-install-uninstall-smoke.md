# NSIS install/run/uninstall Windows smoke

- Date: 2026-09-01
- Conclusion: `go with guardrails`
- Target: Windows x86_64, current-user install mode
- Product/version: Astian `0.0.1`
- Installer: `Astian_0.0.1_x64-setup.exe`
- Size: 2,977,270 bytes
- SHA-256: `46cb24b4ef0a95efe00c5c7ae89a2da5609b5b665d16de13a10599c235820088`
- Authenticode: `NotSigned`

## Preflight and build finding

No Astian uninstall registration or running `astian.exe` existed before the
test. App-local WebView data under `app.astian.desktop` already existed from an
earlier release smoke, so it was explicitly preserved and excluded from fixture
cleanup.

The first build after adding technical-spike binaries selected
`fts_benchmark.exe` as the application. Adding Cargo `default-run = "astian"`
fixed the main executable, but the benchmark was then included as an unwanted
sidecar. Both spike programs were moved from `src/bin` to Cargo `examples`, and
their npm scripts now use `cargo run --example`. Final Cargo metadata contains
one application binary, and the generated NSIS script contains neither spike
executable.

## Procedure and result

The final installer was run silently with `/S /NS` and an absolute `/D=` path
under a dedicated workspace fixture. `/NS` prevented Start Menu and desktop
shortcut creation.

After installation:

- The fixture contained only `astian.exe` (11,664,896 bytes) and
  `uninstall.exe` (79,117 bytes).
- The current-user Add/Remove Programs entry reported display version `0.0.1`
  and its install/uninstall paths pointed exactly to the fixture.
- The installed `astian.exe` was launched hidden for more than five seconds;
  the process path matched the fixture and Windows reported `Responding=True`.
- The process was stopped by its verified PID before uninstall.

The fixture uninstaller was then run with `/S`.

- The complete install directory and Add/Remove Programs entry were removed.
- No Astian process or shortcut remained.
- A Markdown marker outside the install directory retained SHA-256
  `22d0994d70f8360ab3ae2be5ac6869a269fd3bc0b19bf3932672b52dc3e17d7b`.
- Silent uninstall intentionally preserved the app-data preference key. The
  exact product key created by this fixture was removed afterward; pre-existing
  WebView app data was not deleted.

## Guardrails and remaining evidence

- The fixture used the current Windows user and an overridden install path; a
  clean Windows 10/11 VM and default `%LOCALAPPDATA%\\Astian` path remain untested.
- The installer uses Tauri's default WebView2 download-bootstrapper policy. This
  machine already had WebView2, so dependency download/failure was not exercised.
- The artifact is unsigned and may trigger SmartScreen. Code-signing policy is
  still a pre-release decision separate from updater signing.
- Interactive installer pages, uninstall's optional delete-app-data checkbox,
  upgrade/downgrade, reboot, full-disk, permission failure, and N-1 to N update
  remain untested.
