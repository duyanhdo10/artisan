import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";
import {
  MarkdownEditor,
  type MarkdownEditorMode,
} from "./editor/MarkdownEditor";
import {
  formatNoteSize,
  getNoteSizePolicy,
} from "./editor/noteSizePolicy";
import {
  clearRecoveryDraft,
  getRuntimeInfo,
  listRecoveryDrafts,
  normalizeCommandError,
  openNote,
  readRecoveryDraft,
  saveNote,
  saveNoteAsCopy,
  selectVault,
  writeRecoveryDraft,
  type OpenedNote,
  type RecoveryDraftSummary,
  type RuntimeInfo,
  type VaultSummary,
} from "./lib/tauri";

type SaveState = "idle" | "saving" | "failed" | "conflict";
type RecoveryState = "idle" | "pending" | "writing" | "protected" | "failed";

const RECOVERY_DEBOUNCE_MS = 600;

function App() {
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [vault, setVault] = useState<VaultSummary | null>(null);
  const [activeNote, setActiveNote] = useState<OpenedNote | null>(null);
  const [draft, setDraft] = useState("");
  const [operationError, setOperationError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [recoveryState, setRecoveryState] = useState<RecoveryState>("idle");
  const [recoveryDrafts, setRecoveryDrafts] = useState<RecoveryDraftSummary[]>([]);
  const [editorRevision, setEditorRevision] = useState(0);
  const [editorMode, setEditorMode] =
    useState<MarkdownEditorMode>("live-preview");
  const recoveryWriteQueueRef = useRef<Promise<void>>(Promise.resolve());
  const expectedRecoveryHashRef = useRef<string | null>(null);
  const isDirty = activeNote !== null && draft !== activeNote.content;
  const noteSizePolicy = getNoteSizePolicy(draft);
  const livePreviewLimited =
    activeNote !== null && !noteSizePolicy.livePreviewAllowed;
  const effectiveEditorMode = livePreviewLimited ? "source" : editorMode;

  useEffect(() => {
    let active = true;

    getRuntimeInfo()
      .then((info) => {
        if (active) setRuntime(info);
      })
      .catch((error: unknown) => {
        if (active) setRuntimeError(normalizeCommandError(error).message);
      });

    return () => {
      active = false;
    };
  }, []);

  async function handleSelectVault() {
    if (isDirty) {
      setOperationError("Save or reload the modified note before changing vaults.");
      return;
    }

    setBusy(true);
    setOperationError(null);

    try {
      const selectedVault = await selectVault();
      if (selectedVault) {
        setVault(selectedVault);
        setActiveNote(null);
        setDraft("");
        setSaveState("idle");
        setRecoveryState("idle");
        setRecoveryDrafts([]);
        setEditorRevision(0);
        expectedRecoveryHashRef.current = null;
        const pendingRecoveryDrafts = await listRecoveryDrafts();
        setRecoveryDrafts(pendingRecoveryDrafts);
      }
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleOpenNote(relativePath: string) {
    if (relativePath === activeNote?.relativePath) return;
    if (isDirty) {
      setOperationError("Save or reload the modified note before opening another note.");
      return;
    }
    if (recoveryDrafts.some((recovery) => recovery.relativePath === relativePath)) {
      setOperationError(
        "Restore or discard this note's recovery draft before opening it.",
      );
      return;
    }

    setBusy(true);
    setOperationError(null);

    try {
      const note = await openNote(relativePath);
      setActiveNote(note);
      setDraft(note.content);
      setSaveState("idle");
      setRecoveryState("idle");
      setEditorRevision(0);
      expectedRecoveryHashRef.current = null;
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  const handleSave = useCallback(async () => {
    if (
      !activeNote ||
      !isDirty ||
      saveState === "saving" ||
      saveState === "conflict"
    ) {
      return;
    }

    const notePath = activeNote.relativePath;
    const contentToSave = draft;
    const expectedHash = activeNote.contentHash;
    setSaveState("saving");
    setOperationError(null);

    try {
      const result = await saveNote(notePath, contentToSave, expectedHash);
      setActiveNote((current) =>
        current?.relativePath === notePath
          ? {
              ...current,
              content: contentToSave,
              contentHash: result.contentHash,
            }
          : current,
      );
      setSaveState("idle");
      setRecoveryState("idle");
      setRecoveryDrafts((current) =>
        current.filter((recovery) => recovery.relativePath !== notePath),
      );
      expectedRecoveryHashRef.current = null;
    } catch (error: unknown) {
      const commandError = normalizeCommandError(error);
      setSaveState(
        commandError.code === "external_change_conflict" ? "conflict" : "failed",
      );
      setOperationError(commandError.message);
    }
  }, [activeNote, draft, isDirty, saveState]);

  async function handleReload() {
    if (!activeNote || busy) return;

    setBusy(true);
    setOperationError(null);
    try {
      if (expectedRecoveryHashRef.current !== null) {
        await clearRecoveryDraft(activeNote.relativePath);
      }
      const note = await openNote(activeNote.relativePath);
      setActiveNote(note);
      setDraft(note.content);
      setSaveState("idle");
      setRecoveryState("idle");
      setEditorRevision(0);
      expectedRecoveryHashRef.current = null;
    } catch (error: unknown) {
      setSaveState("failed");
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void handleSave();
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleSave]);

  async function handleRestoreRecovery(summary: RecoveryDraftSummary) {
    if (busy || isDirty) return;

    setBusy(true);
    setOperationError(null);
    try {
      const [note, recovery] = await Promise.all([
        openNote(summary.relativePath),
        readRecoveryDraft(summary.relativePath),
      ]);
      setActiveNote(note);
      setDraft(recovery.content);
      setEditorRevision(recovery.editorRevision);
      if (note.content === recovery.content) {
        await clearRecoveryDraft(summary.relativePath);
        expectedRecoveryHashRef.current = null;
        setRecoveryState("idle");
        setSaveState("idle");
        setRecoveryDrafts((current) =>
          current.filter((draftSummary) => draftSummary.relativePath !== summary.relativePath),
        );
        return;
      }
      expectedRecoveryHashRef.current = recovery.contentHash;
      setRecoveryState("protected");
      setSaveState(
        note.contentHash === recovery.baseHash ? "idle" : "conflict",
      );
      setRecoveryDrafts((current) =>
        current.filter((draftSummary) => draftSummary.relativePath !== summary.relativePath),
      );
      if (note.contentHash !== recovery.baseHash) {
        setOperationError(
          "The disk note changed after this recovery draft. Save As Copy is required before replacing either version.",
        );
      }
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleDiscardRecovery(summary: RecoveryDraftSummary) {
    if (busy || isDirty) return;

    setBusy(true);
    setOperationError(null);
    try {
      await clearRecoveryDraft(summary.relativePath);
      setRecoveryDrafts((current) =>
        current.filter((draftSummary) => draftSummary.relativePath !== summary.relativePath),
      );
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleSaveAsCopy(summary?: RecoveryDraftSummary) {
    if (busy || (summary !== undefined && isDirty)) return;

    const sourceRelativePath = summary?.relativePath ?? activeNote?.relativePath;
    if (!sourceRelativePath) return;

    setBusy(true);
    setOperationError(null);
    try {
      const contentToCopy = summary
        ? (await readRecoveryDraft(summary.relativePath)).content
        : draft;
      const copied = await saveNoteAsCopy(sourceRelativePath, contentToCopy);
      if (!copied) return;

      setActiveNote(copied);
      setDraft(copied.content);
      setEditorRevision(0);
      setSaveState("idle");
      setRecoveryState("idle");
      expectedRecoveryHashRef.current = null;
      setRecoveryDrafts((current) =>
        current.filter(
          (draftSummary) => draftSummary.relativePath !== sourceRelativePath,
        ),
      );
      setVault((current) => {
        if (!current) return current;
        const fileName = copied.relativePath.split("/").pop();
        const title = fileName?.replace(/\.md$/i, "") ?? copied.relativePath;
        return {
          ...current,
          notes: [...current.notes, { relativePath: copied.relativePath, title }]
            .sort((left, right) =>
              left.relativePath.localeCompare(right.relativePath),
            ),
        };
      });
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    if (!activeNote || !isDirty || activeNote.lineEnding === "mixed") {
      setRecoveryState("idle");
      return;
    }

    let cancelled = false;
    const notePath = activeNote.relativePath;
    const contentToProtect = draft;
    const baseHash = activeNote.contentHash;
    const revisionToProtect = editorRevision;
    setRecoveryState("pending");

    const timer = window.setTimeout(() => {
      if (!cancelled) setRecoveryState("writing");

      const write = recoveryWriteQueueRef.current
        .catch(() => undefined)
        .then(() =>
          writeRecoveryDraft(
            notePath,
            contentToProtect,
            baseHash,
            revisionToProtect,
            expectedRecoveryHashRef.current,
          ),
        );
      recoveryWriteQueueRef.current = write.then(
        () => undefined,
        () => undefined,
      );

      write
        .then((summary) => {
          expectedRecoveryHashRef.current = summary.contentHash;
          if (!cancelled && summary.editorRevision === revisionToProtect) {
            setRecoveryState("protected");
          }
        })
        .catch((error: unknown) => {
          if (cancelled) return;
          const commandError = normalizeCommandError(error);
          setRecoveryState("failed");
          if (commandError.code === "recovery_base_changed") {
            setSaveState("conflict");
          }
          setOperationError(commandError.message);
        });
    }, RECOVERY_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [activeNote, draft, editorRevision, isDirty]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    getCurrentWindow()
      .onCloseRequested((event) => {
        if (isDirty || saveState === "saving") {
          event.preventDefault();
          setOperationError(
            saveState === "saving"
              ? "Wait for the current save to finish before closing Astian."
              : "Save or reload the modified note before closing Astian.",
          );
        }
      })
      .then((stopListening) => {
        if (disposed) stopListening();
        else unlisten = stopListening;
      })
      .catch(() => {
        // Browser-only frontend previews do not expose a native window.
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [isDirty, saveState]);

  const isMixedLineEnding = activeNote?.lineEnding === "mixed";
  const saveLabel =
    saveState === "saving"
      ? "Saving…"
      : saveState === "conflict"
        ? "External conflict"
        : saveState === "failed"
          ? "Save failed"
          : activeNote
            ? isMixedLineEnding
              ? "Mixed line endings · read-only"
              : isDirty
                ? "Modified"
                : `Saved · ${activeNote.contentHash.slice(0, 8)}`
            : "Open a note to start";
  const recoveryLabel =
    recoveryState === "pending"
      ? "Recovery pending"
      : recoveryState === "writing"
        ? "Protecting draft…"
        : recoveryState === "protected"
          ? "Draft protected"
          : recoveryState === "failed"
            ? "Recovery failed"
            : null;

  const handleDraftChange = useCallback((value: string) => {
    setDraft(value);
    setEditorRevision((revision) => revision + 1);
  }, []);

  return (
    <div className="app-shell">
      <aside className="navigation" aria-label="Primary navigation">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">A</span>
          <div>
            <strong>Astian</strong>
            <span>Local-first notes</span>
          </div>
        </div>

        <button
          className="primary-action"
          type="button"
          onClick={handleSelectVault}
          disabled={busy || runtime === null}
        >
          {busy ? "Working…" : vault ? "Change vault" : "Open vault"}
        </button>

        <nav className="nav-sections" aria-label="Vault tools">
          <button className="nav-item active" type="button">
            <span aria-hidden="true">▤</span> Files
          </button>
          <button className="nav-item" type="button" disabled>
            <span aria-hidden="true">⌕</span> Search
          </button>
          <button className="nav-item" type="button" disabled>
            <span aria-hidden="true">#</span> Tags
          </button>
          <button className="nav-item" type="button" disabled>
            <span aria-hidden="true">⑂</span> Git
          </button>
        </nav>

        <section className="file-browser" aria-label="Vault Markdown files">
          {vault ? (
            <>
              <div className="vault-heading">
                <strong>{vault.name}</strong>
                <span>{vault.notes.length} notes</span>
              </div>
              {vault.notes.length > 0 ? (
                <div className="note-list">
                  {vault.notes.map((note) => (
                    <button
                      className={
                        activeNote?.relativePath === note.relativePath
                          ? "note-item active"
                          : "note-item"
                      }
                      key={note.relativePath}
                      type="button"
                      title={note.relativePath}
                      onClick={() => handleOpenNote(note.relativePath)}
                      disabled={busy}
                    >
                      <span aria-hidden="true">◇</span>
                      <span>{note.title}</span>
                    </button>
                  ))}
                </div>
              ) : (
                <div className="empty-vault">
                  <span className="folder-icon" aria-hidden="true">◇</span>
                  <p>No Markdown notes</p>
                  <small>Add a `.md` file to this folder and reopen the vault.</small>
                </div>
              )}
            </>
          ) : (
            <div className="empty-vault">
              <span className="folder-icon" aria-hidden="true">◇</span>
              <p>No vault is open</p>
              <small>Choose a folder that contains your Markdown files.</small>
            </div>
          )}
        </section>

        <div className="sidebar-footer">
          {runtime ? (
            <span>v{runtime.appVersion} · {runtime.architecture}</span>
          ) : runtimeError ? (
            <span title={runtimeError}>Frontend preview</span>
          ) : (
            <span>Connecting to native layer…</span>
          )}
        </div>
      </aside>

      <main className="workspace">
        <header className="tab-bar">
          <button className="tab active" type="button">
            {activeNote?.relativePath ?? "Welcome"}
            {isDirty ? <span className="dirty-dot" aria-label="Modified">●</span> : null}
          </button>
          <div className="window-drag-region" data-tauri-drag-region />
        </header>

        <section
          className={
            recoveryDrafts.length > 0
              ? "editor-pane has-recovery"
              : "editor-pane"
          }
          aria-label="Markdown editor spike"
        >
          <div className="editor-toolbar">
            <div className="editor-mode-group">
              <div className="editor-mode" aria-label="Editor mode">
                <button
                  className={effectiveEditorMode === "live-preview" ? "active" : ""}
                  type="button"
                  aria-pressed={effectiveEditorMode === "live-preview"}
                  aria-describedby={livePreviewLimited ? "note-size-warning" : undefined}
                  title={
                    livePreviewLimited
                      ? "Live Preview is unavailable above the 512 KiB soft limit."
                      : undefined
                  }
                  onClick={() => setEditorMode("live-preview")}
                  disabled={livePreviewLimited}
                >
                  Live Preview
                </button>
                <button
                  className={effectiveEditorMode === "source" ? "active" : ""}
                  type="button"
                  aria-pressed={effectiveEditorMode === "source"}
                  onClick={() => setEditorMode("source")}
                >
                  Source
                </button>
              </div>
              {livePreviewLimited ? (
                <span
                  className="note-size-warning"
                  id="note-size-warning"
                  role="status"
                >
                  Large note ({formatNoteSize(noteSizePolicy.utf8Bytes)}) · Source mode
                </span>
              ) : null}
            </div>
            <div className="save-controls">
              <span
                className={
                  saveState === "conflict" || saveState === "failed" || isDirty
                    ? "save-state warning"
                    : "save-state"
                }
              >
                {saveLabel}
              </span>
              {recoveryLabel ? (
                <span
                  className={
                    recoveryState === "failed"
                      ? "save-state warning"
                      : "save-state"
                  }
                >
                  {recoveryLabel}
                </span>
              ) : null}
              {saveState === "conflict" || saveState === "failed" ? (
                <button
                  className="secondary-action"
                  type="button"
                  onClick={() => handleSaveAsCopy()}
                  disabled={busy}
                >
                  Save As Copy
                </button>
              ) : null}
              {saveState === "conflict" ? (
                  <button className="secondary-action" type="button" onClick={handleReload}>
                    Reload disk version
                  </button>
              ) : null}
              <button
                className="save-action"
                type="button"
                onClick={handleSave}
                disabled={
                  !isDirty ||
                  isMixedLineEnding ||
                  saveState === "saving" ||
                  saveState === "conflict"
                }
              >
                {saveState === "failed" ? "Retry save" : "Save"}
              </button>
            </div>
          </div>
          {recoveryDrafts.length > 0 ? (
            <div className="recovery-banner" role="status">
              <strong>Unsaved recovery drafts found</strong>
              {recoveryDrafts.map((summary) => (
                <div className="recovery-item" key={summary.relativePath}>
                  <span>{summary.relativePath}</span>
                  <button
                    type="button"
                    onClick={() => handleRestoreRecovery(summary)}
                    disabled={busy || isDirty}
                  >
                    Restore
                  </button>
                  <button
                    type="button"
                    onClick={() => handleSaveAsCopy(summary)}
                    disabled={busy || isDirty}
                  >
                    Save As Copy
                  </button>
                  <button
                    className="discard-recovery"
                    type="button"
                    onClick={() => handleDiscardRecovery(summary)}
                    disabled={busy || isDirty}
                  >
                    Discard
                  </button>
                </div>
              ))}
            </div>
          ) : null}
          {operationError ? <div className="error-banner" role="alert">{operationError}</div> : null}
          <MarkdownEditor
            key={activeNote?.relativePath ?? "no-note"}
            ariaLabel="Markdown source"
            value={draft}
            onChange={handleDraftChange}
            mode={effectiveEditorMode}
            disabled={activeNote === null}
            readOnly={isMixedLineEnding}
          />
        </section>
      </main>

      <aside className="context-panel" aria-label="Note context">
        <header>
          <strong>Backlinks</strong>
          <button type="button" aria-label="Collapse context panel" disabled>›</button>
        </header>
        <div className="context-empty">
          <span aria-hidden="true">↙</span>
          <p>No backlinks yet</p>
          <small>Links to the active note will appear here.</small>
        </div>
        <section className="properties">
          <h2>Properties</h2>
          <p>No frontmatter properties.</p>
        </section>
      </aside>
    </div>
  );
}

export default App;
