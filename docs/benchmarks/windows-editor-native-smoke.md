# Windows native editor smoke

- Date: 2026-09-01
- Baseline build: release `astian.exe` from commit `66bcc6f`; the close-lifecycle
  follow-up used a release rebuild containing the fix documented below
- Host: Windows `10.0.26300.9278`, Intel Core i5-8300H, 16 GB RAM
- Scope: Tauri/WebView2 UI, CodeMirror editing, watcher/conflict, safe-save, and the 512 KiB guardrail

## Method

The smoke used a disposable vault under the repository workspace. The small
fixture contained Markdown syntax, Vietnamese text, and emoji. A valid UTF-8
production JavaScript bundle was copied to `large.md` to provide a deterministic
543,172-byte note above the provisional 512 KiB Live Preview limit.

The release application was driven through Windows UI Automation and verified
with native window captures. Keystroke timing is the elapsed time from one
`SendKeys` character through observing the updated WebView2 accessibility value;
it therefore includes automation and accessibility round-trip overhead rather
than representing a browser `input` event measurement alone.

The in-app Browser runtime was also queried, but no in-app or external browser
instance was connected. Browser-based DOM instrumentation was not substituted
or claimed.

## Results

### Startup and host-process memory

- First visible main-window handle: 3,213.10 ms after `Start-Process`.
- Astian host process at first window: 21.01 MB working set, 3.29 MB private memory.
- After opening the small vault: 72.57 MB working set, 13.03 MB private memory.
- With the large note open: 64.03 MB working set, 11.40 MB private memory.

These memory figures cover `astian.exe` only and do not include separate WebView2
child processes. They are directional spike evidence, not a total application
memory budget.

### Live Preview, selection, and Source mode

- The small fixture rendered its ATX heading and strong emphasis without
  changing the Markdown file.
- A real mouse drag selected `bản nháp kiểm ` in the strong-emphasis node.
  CodeMirror revealed both `**` marker pairs while preserving the selection.
- Windows UI Automation reported Source `On` and Live Preview `Off` after the
  mode switch. The selection remained visible, and switching back restored Live
  Preview.

### Typing, undo/redo, and save

Twenty-two characters were typed into the release WebView2 editor:

- round-trip p50: 27.74 ms;
- round-trip p95: 49.87 ms;
- maximum: 52.36 ms;
- one undo restored the exact pre-edit accessibility value;
- one redo restored the complete edit;
- `Ctrl+S` changed the fixture hash from prefix `3604cf01` to `1647c8fe`;
- the UI reported `Saved · 1647c8fe`, and the disk file contained the full suffix.

The p95 meets the spike's provisional 50 ms long-task budget. The maximum is
slightly above it, so this is still a `go with guardrails` result rather than a
production performance guarantee.

### Vietnamese IME composition

The installed Windows Vietnamese input profile was selected while Astian had
editor focus. Typing the Telex sequence `Tieengs Vieetj` produced `Tiếng Việt`
inside CodeMirror. A single undo restored the buffer and the disk hash remained
unchanged. The input profile was switched back and verified by observing
`Tieengs` remain raw under the English profile; that verification edit was also
undone.

### Large-note fallback

- The watcher detected the newly copied 543,172-byte `large.md` without reopening
  the vault.
- Opening the note and reaching the guarded state took 197.71 ms.
- Live Preview became disabled, Source became active, and the UI displayed
  `Large note (531 KiB) · Source mode`.
- Returning to the small note automatically restored the previous Live Preview
  preference.

The 512 KiB threshold remains a soft presentation guardrail. The note stayed
openable and editable; no format or save restriction was introduced.

### Dirty/external conflict

The editor was kept dirty with repeated input while an external patch changed
the same fixture on disk. The watcher moved the UI to `External conflict`:

- Save was disabled;
- Save As Copy and Reload disk version were enabled;
- the external file hash was not overwritten;
- Reload disk version cleared the conflict, loaded the external line, and
  discarded the local-only repeated input.

Recovery reported that the note changed before the pending recovery draft could
be protected. This is the expected fail-closed result for the injected race;
the app did not claim a protected draft or silently overwrite either version.

### Window close lifecycle

The first clean-close attempt exposed a Tauri capability defect: the frontend
close guard received the native close event, but the main-window capability did
not grant `core:window:allow-destroy` for the listener's clean-close completion
or `core:window:allow-close` for the post-save close request. The listener was
also re-registered whenever editor state changed, which made stale listeners a
possible race.

The follow-up grants only those two window permissions, moves close decisions
into a tested pure policy, and keeps one stable native listener backed by current
state refs. A rebuilt release binary then passed both native paths:

- a real title-bar close on a clean window exited the process in 83.17 ms;
- a real title-bar close issued about 45 ms after an unsaved keystroke flushed the
  buffer and exited the process in 113.62 ms;
- the dirty-close fixture SHA-256 changed from `baeea6aa…` to `2e63ac24…`, and
  the typed `x` was present on disk after process exit.

The dirty-close test deliberately beat the 700 ms autosave debounce, so the
write was caused by the close guard rather than ordinary autosave.

## Remaining gaps

- Browser-based `PerformanceObserver`/event-to-paint instrumentation was not
  available because no Browser instance was connected. The UI Automation timing
  above is an end-to-end upper-bound proxy.
- Host-process memory does not include the WebView2 process tree, so a total
  application memory profile remains follow-up work rather than a merge blocker
  for this architecture spike.
- The disposable vault was removed after the smoke. Window captures remained
  temporary local artifacts and were not committed.

## Disposition

CodeMirror 6/CommonMark remains **go with guardrails**. Native selection reveal,
Source/Live mode switching, Vietnamese IME composition, undo/redo, safe-save,
large-note fallback, and dirty/external conflict behavior all have visible
Windows evidence. Clean and dirty native close paths now pass as well. No known
editor-architecture blocker remains for merging the technical-spike branch;
browser event-to-paint and total WebView2-process memory stay as explicit
profiling guardrails for subsequent work.
