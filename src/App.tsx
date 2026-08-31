import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";
import {
  MarkdownEditor,
  type MarkdownEditorMode,
} from "./editor/MarkdownEditor";
import {
  getRuntimeInfo,
  normalizeCommandError,
  openNote,
  saveNote,
  selectVault,
  type OpenedNote,
  type RuntimeInfo,
  type VaultSummary,
} from "./lib/tauri";

type SaveState = "idle" | "saving" | "failed" | "conflict";

function App() {
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [vault, setVault] = useState<VaultSummary | null>(null);
  const [activeNote, setActiveNote] = useState<OpenedNote | null>(null);
  const [draft, setDraft] = useState("");
  const [operationError, setOperationError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [editorMode, setEditorMode] =
    useState<MarkdownEditorMode>("live-preview");
  const isDirty = activeNote !== null && draft !== activeNote.content;

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

    setBusy(true);
    setOperationError(null);

    try {
      const note = await openNote(relativePath);
      setActiveNote(note);
      setDraft(note.content);
      setSaveState("idle");
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
      const note = await openNote(activeNote.relativePath);
      setActiveNote(note);
      setDraft(note.content);
      setSaveState("idle");
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

        <section className="editor-pane" aria-label="Markdown editor spike">
          <div className="editor-toolbar">
            <div className="editor-mode" aria-label="Editor mode">
              <button
                className={editorMode === "live-preview" ? "active" : ""}
                type="button"
                aria-pressed={editorMode === "live-preview"}
                onClick={() => setEditorMode("live-preview")}
              >
                Live Preview
              </button>
              <button
                className={editorMode === "source" ? "active" : ""}
                type="button"
                aria-pressed={editorMode === "source"}
                onClick={() => setEditorMode("source")}
              >
                Source
              </button>
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
          {operationError ? <div className="error-banner" role="alert">{operationError}</div> : null}
          <MarkdownEditor
            key={activeNote?.relativePath ?? "no-note"}
            ariaLabel="Markdown source"
            value={draft}
            onChange={setDraft}
            mode={editorMode}
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
