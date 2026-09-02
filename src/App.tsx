import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";
import type { MarkdownEditorMode } from "./editor/MarkdownEditor";
import {
  AUTOSAVE_DEBOUNCE_MS,
  saveAfterRecoveryQueue,
  shouldQueueAutosave,
  type AutosaveSaveState,
} from "./editor/autosavePolicy";
import {
  decideWindowClose,
  type WindowCloseContext,
} from "./editor/closePolicy";
import {
  formatNoteSize,
  getNoteSizePolicy,
} from "./editor/noteSizePolicy";
import { decideActiveNoteWatcherAction } from "./editor/watcherPolicy";
import {
  clearRecoveryDraft,
  deleteUnavailableRecoveryDraft,
  exportUnavailableRecoveryDraft,
  getRuntimeInfo,
  listenVaultChanges,
  listRecoveryDrafts,
  normalizeCommandError,
  openNote,
  readRecoveryDraft,
  reconcileVault,
  restoreLastVault,
  saveNote,
  saveNoteAsCopy,
  selectVault,
  writeRecoveryDraft,
  type OpenedNote,
  type RecoveryDraftSummary,
  type RuntimeInfo,
  type UnavailableRecoveryDraft,
  type VaultWatcherEvent,
  type VaultSummary,
} from "./lib/tauri";

const MarkdownEditor = lazy(() =>
  import("./editor/MarkdownEditor").then((module) => ({
    default: module.MarkdownEditor,
  })),
);

type SaveState = AutosaveSaveState;
type RecoveryState = "idle" | "pending" | "writing" | "protected" | "failed";

const RECOVERY_DEBOUNCE_MS = 600;

function App() {
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [vault, setVault] = useState<VaultSummary | null>(null);
  const [activeNote, setActiveNote] = useState<OpenedNote | null>(null);
  const [draft, setDraft] = useState("");
  const [operationError, setOperationError] = useState<string | null>(null);
  const [watcherNotice, setWatcherNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [recoveryState, setRecoveryState] = useState<RecoveryState>("idle");
  const [recoveryDrafts, setRecoveryDrafts] = useState<RecoveryDraftSummary[]>([]);
  const [unavailableRecoveryDrafts, setUnavailableRecoveryDrafts] = useState<
    UnavailableRecoveryDraft[]
  >([]);
  const [recoveryNotice, setRecoveryNotice] = useState<string | null>(null);
  const [pendingRecoveryDeleteId, setPendingRecoveryDeleteId] = useState<
    string | null
  >(null);
  const [editorRevision, setEditorRevision] = useState(0);
  const [editorMode, setEditorMode] =
    useState<MarkdownEditorMode>("live-preview");
  const recoveryWriteQueueRef = useRef<Promise<void>>(Promise.resolve());
  const expectedRecoveryHashRef = useRef<string | null>(null);
  const allowWindowCloseRef = useRef(false);
  const closeAfterSaveRef = useRef(false);
  const closeContextRef = useRef<WindowCloseContext>({
    isDirty: false,
    isBusy: false,
    hasMixedLineEndings: false,
    saveState: "idle",
  });
  const saveForCloseRef = useRef<() => Promise<boolean>>(() =>
    Promise.resolve(false),
  );
  const vaultRef = useRef<VaultSummary | null>(null);
  const activeNoteRef = useRef<OpenedNote | null>(null);
  const draftRef = useRef("");
  const editorRevisionRef = useRef(0);
  const watcherEventQueueRef = useRef<Promise<void>>(Promise.resolve());
  const lastWatcherEventRef = useRef({ vaultSession: 0, revision: 0 });
  const isDirty = activeNote !== null && draft !== activeNote.content;
  const noteSizePolicy = getNoteSizePolicy(draft);
  const livePreviewLimited =
    activeNote !== null && !noteSizePolicy.livePreviewAllowed;
  const effectiveEditorMode = livePreviewLimited ? "source" : editorMode;

  vaultRef.current = vault;
  activeNoteRef.current = activeNote;
  draftRef.current = draft;
  editorRevisionRef.current = editorRevision;
  closeContextRef.current = {
    isDirty,
    isBusy: busy,
    hasMixedLineEndings: activeNote?.lineEnding === "mixed",
    saveState,
  };

  useEffect(() => {
    let active = true;

    getRuntimeInfo()
      .then(async (info) => {
        if (!active) return;
        setRuntime(info);
        setBusy(true);
        try {
          const restoredVault = await restoreLastVault();
          if (active && restoredVault) await applyVault(restoredVault);
        } catch (error: unknown) {
          if (active) setOperationError(normalizeCommandError(error).message);
        } finally {
          if (active) setBusy(false);
        }
      })
      .catch((error: unknown) => {
        if (active) setRuntimeError(normalizeCommandError(error).message);
      });

    return () => {
      active = false;
    };
  }, []);

  async function applyVault(nextVault: VaultSummary) {
    vaultRef.current = nextVault;
    activeNoteRef.current = null;
    draftRef.current = "";
    editorRevisionRef.current = 0;
    lastWatcherEventRef.current = {
      vaultSession: nextVault.vaultSession,
      revision: 0,
    };
    setVault(nextVault);
    setActiveNote(null);
    setDraft("");
    setSaveState("idle");
    setRecoveryState("idle");
    setRecoveryDrafts([]);
    setUnavailableRecoveryDrafts([]);
    setRecoveryNotice(null);
    setPendingRecoveryDeleteId(null);
    setEditorRevision(0);
    expectedRecoveryHashRef.current = null;
    const recoveryItems = await listRecoveryDrafts();
    setRecoveryDrafts(
      recoveryItems.flatMap((item) =>
        item.status === "available" ? [item.draft] : [],
      ),
    );
    setUnavailableRecoveryDrafts(
      recoveryItems.filter(
        (item): item is UnavailableRecoveryDraft => item.status === "unavailable",
      ),
    );
    await reconcileVault();
  }

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    async function processWatcherEvent(event: VaultWatcherEvent) {
      if (disposed) return;
      const currentVault = vaultRef.current;
      if (!currentVault || event.vaultSession !== currentVault.vaultSession) {
        return;
      }
      const lastEvent = lastWatcherEventRef.current;
      if (
        lastEvent.vaultSession === event.vaultSession &&
        event.revision <= lastEvent.revision
      ) {
        return;
      }
      lastWatcherEventRef.current = {
        vaultSession: event.vaultSession,
        revision: event.revision,
      };

      if (event.status === "rescan_required") {
        setWatcherNotice(null);
        setOperationError(
          "Vault changes could not be reconciled yet. Astian will retry when the window regains focus.",
        );
        return;
      }

      const nextVault = { ...currentVault, notes: event.notes };
      vaultRef.current = nextVault;
      setVault((current) =>
        current?.vaultSession === event.vaultSession ? nextVault : current,
      );

      const currentNote = activeNoteRef.current;
      const dirty =
        currentNote !== null && draftRef.current !== currentNote.content;
      const action = decideActiveNoteWatcherAction(
        event.changes,
        currentNote?.relativePath ?? null,
        dirty,
      );
      if (action.kind === "none") return;
      if (action.kind === "conflict") {
        setWatcherNotice(null);
        setSaveState("conflict");
        setOperationError(
          "The active note changed outside Astian while local edits were pending. Autosave was stopped.",
        );
        return;
      }
      if (action.kind === "close") {
        activeNoteRef.current = null;
        draftRef.current = "";
        editorRevisionRef.current = 0;
        setActiveNote(null);
        setDraft("");
        setEditorRevision(0);
        setSaveState("idle");
        setRecoveryState("idle");
        expectedRecoveryHashRef.current = null;
        setOperationError(null);
        setWatcherNotice("The active note was deleted outside Astian.");
        return;
      }

      const observedPath = currentNote?.relativePath;
      const observedRevision = editorRevisionRef.current;
      try {
        const reloaded = await openNote(action.relativePath);
        const latestActiveNote = activeNoteRef.current;
        if (
          disposed ||
          !latestActiveNote ||
          latestActiveNote.relativePath !== observedPath
        ) {
          return;
        }
        const changedWhileReloading =
          editorRevisionRef.current !== observedRevision ||
          draftRef.current !== latestActiveNote.content;
        if (changedWhileReloading) {
          setWatcherNotice(null);
          setSaveState("conflict");
          setOperationError(
            "The active note changed outside Astian while local edits were pending. Autosave was stopped.",
          );
          return;
        }

        activeNoteRef.current = reloaded;
        draftRef.current = reloaded.content;
        editorRevisionRef.current = 0;
        setActiveNote(reloaded);
        setDraft(reloaded.content);
        setEditorRevision(0);
        setSaveState("idle");
        setRecoveryState("idle");
        expectedRecoveryHashRef.current = null;
        setOperationError(null);
        setWatcherNotice(
          action.relativePath === observedPath
            ? "The active note was reloaded after an external change."
            : `The active note was renamed outside Astian to ${action.relativePath}.`,
        );
      } catch (error: unknown) {
        setWatcherNotice(null);
        setOperationError(normalizeCommandError(error).message);
      }
    }

    listenVaultChanges((event) => {
      watcherEventQueueRef.current = watcherEventQueueRef.current
        .catch(() => undefined)
        .then(() => processWatcherEvent(event));
    })
      .then((stopListening) => {
        if (disposed) stopListening();
        else unlisten = stopListening;
      })
      .catch(() => {
        // Browser-only frontend previews do not expose native events.
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    function handleWindowFocus() {
      if (!vaultRef.current) return;
      void reconcileVault().catch((error: unknown) => {
        setOperationError(normalizeCommandError(error).message);
      });
    }

    window.addEventListener("focus", handleWindowFocus);
    return () => window.removeEventListener("focus", handleWindowFocus);
  }, []);

  async function handleSelectVault() {
    if (isDirty) {
      const saved = await handleSave();
      if (!saved) return;
    }

    setBusy(true);
    setOperationError(null);
    setWatcherNotice(null);

    try {
      const selectedVault = await selectVault();
      if (selectedVault) {
        await applyVault(selectedVault);
      }
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleOpenNote(relativePath: string) {
    if (relativePath === activeNote?.relativePath) return;
    if (recoveryDrafts.some((recovery) => recovery.relativePath === relativePath)) {
      setOperationError(
        "Restore or discard this note's recovery draft before opening it.",
      );
      return;
    }
    if (isDirty) {
      const saved = await handleSave();
      if (!saved) return;
    }

    setBusy(true);
    setOperationError(null);
    setWatcherNotice(null);

    try {
      const note = await openNote(relativePath);
      activeNoteRef.current = note;
      draftRef.current = note.content;
      editorRevisionRef.current = 0;
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

  const handleSave = useCallback(async (): Promise<boolean> => {
    if (!activeNote || !isDirty) return true;
    if (
      busy ||
      activeNote.lineEnding === "mixed" ||
      saveState === "saving" ||
      saveState === "conflict"
    ) {
      return false;
    }

    const notePath = activeNote.relativePath;
    const contentToSave = draft;
    const expectedHash = activeNote.contentHash;
    setSaveState("saving");
    setOperationError(null);
    setWatcherNotice(null);

    try {
      const result = await saveNote(notePath, contentToSave, expectedHash);
      if (activeNoteRef.current?.relativePath === notePath) {
        activeNoteRef.current = {
          ...activeNoteRef.current,
          content: contentToSave,
          contentHash: result.contentHash,
        };
      }
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
      return true;
    } catch (error: unknown) {
      const commandError = normalizeCommandError(error);
      closeAfterSaveRef.current = false;
      setSaveState(
        commandError.code === "external_change_conflict" ? "conflict" : "failed",
      );
      setOperationError(commandError.message);
      return false;
    }
  }, [activeNote, busy, draft, isDirty, saveState]);

  saveForCloseRef.current = handleSave;

  async function handleReload() {
    if (!activeNote || busy) return;

    setBusy(true);
    setOperationError(null);
    setWatcherNotice(null);
    try {
      if (expectedRecoveryHashRef.current !== null) {
        await clearRecoveryDraft(activeNote.relativePath);
      }
      const note = await openNote(activeNote.relativePath);
      activeNoteRef.current = note;
      draftRef.current = note.content;
      editorRevisionRef.current = 0;
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
    let blurTimer: number | undefined;

    function handleKeyDown(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void handleSave();
      }
    }

    function handleWindowBlur() {
      blurTimer = window.setTimeout(() => void handleSave(), 0);
    }

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("blur", handleWindowBlur);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("blur", handleWindowBlur);
      if (blurTimer !== undefined) window.clearTimeout(blurTimer);
    };
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
      activeNoteRef.current = note;
      draftRef.current = recovery.content;
      editorRevisionRef.current = recovery.editorRevision;
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

      activeNoteRef.current = copied;
      draftRef.current = copied.content;
      editorRevisionRef.current = 0;
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

  async function handleExportUnavailableRecovery(
    recovery: UnavailableRecoveryDraft,
  ) {
    if (busy) return;

    setBusy(true);
    setOperationError(null);
    setRecoveryNotice(null);
    try {
      const exported = await exportUnavailableRecoveryDraft(
        recovery.recoveryId,
        recovery.artifactHash,
      );
      if (exported) {
        setRecoveryNotice(
          `Recovery data ${recovery.recoveryId.slice(0, 8)} was exported.`,
        );
      }
    } catch (error: unknown) {
      setOperationError(normalizeCommandError(error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleDeleteUnavailableRecovery(
    recovery: UnavailableRecoveryDraft,
  ) {
    if (busy) return;
    if (pendingRecoveryDeleteId !== recovery.recoveryId) {
      setPendingRecoveryDeleteId(recovery.recoveryId);
      setRecoveryNotice(
        "This cannot be undone. Export first if the data may still be useful, then confirm delete.",
      );
      return;
    }

    setBusy(true);
    setOperationError(null);
    setRecoveryNotice(null);
    try {
      await deleteUnavailableRecoveryDraft(
        recovery.recoveryId,
        recovery.artifactHash,
      );
      setUnavailableRecoveryDrafts((current) =>
        current.filter((item) => item.recoveryId !== recovery.recoveryId),
      );
      setPendingRecoveryDeleteId(null);
      setRecoveryNotice("Unavailable recovery data was deleted.");
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
    const shouldQueue = shouldQueueAutosave({
      hasActiveNote: activeNote !== null,
      isDirty,
      isBusy: busy,
      hasMixedLineEndings: activeNote?.lineEnding === "mixed",
      saveState,
    });
    if (!shouldQueue) {
      if (saveState === "queued") setSaveState("idle");
      return;
    }
    if (saveState === "idle") {
      setSaveState("queued");
      return;
    }

    let cancelled = false;
    const timer = window.setTimeout(() => {
      void saveAfterRecoveryQueue(
        recoveryWriteQueueRef.current,
        () => cancelled,
        handleSave,
      );
    }, AUTOSAVE_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [activeNote, busy, draft, editorRevision, handleSave, isDirty, saveState]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    getCurrentWindow()
      .onCloseRequested((event) => {
        if (allowWindowCloseRef.current) return;
        const decision = decideWindowClose(closeContextRef.current);
        if (decision.kind === "allow") return;

        event.preventDefault();
        setOperationError(decision.message);
        if (decision.kind === "block") return;

        closeAfterSaveRef.current = true;
        if (decision.shouldStartSave) {
          void saveForCloseRef.current().then((saved) => {
            if (!saved) closeAfterSaveRef.current = false;
          });
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
  }, []);

  useEffect(() => {
    if (
      !closeAfterSaveRef.current ||
      isDirty ||
      saveState !== "idle"
    ) {
      return;
    }

    closeAfterSaveRef.current = false;
    allowWindowCloseRef.current = true;
    getCurrentWindow()
      .close()
      .catch(() => {
        allowWindowCloseRef.current = false;
        setOperationError("Astian could not close the native window.");
      });
  }, [isDirty, saveState]);

  const isMixedLineEnding = activeNote?.lineEnding === "mixed";
  const saveLabel =
    saveState === "saving"
      ? "Saving…"
      : saveState === "queued"
        ? "Autosave queued"
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
  const hasRecoveryItems =
    recoveryDrafts.length > 0 || unavailableRecoveryDrafts.length > 0;

  const handleDraftChange = useCallback((value: string) => {
    draftRef.current = value;
    editorRevisionRef.current += 1;
    setDraft(value);
    setEditorRevision((revision) => revision + 1);
    setSaveState((current) => (current === "failed" ? "idle" : current));
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
            hasRecoveryItems
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
                role="status"
                aria-live="polite"
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
                  busy ||
                  isMixedLineEnding ||
                  saveState === "saving" ||
                  saveState === "conflict"
                }
              >
                {saveState === "failed" ? "Retry save" : "Save"}
              </button>
            </div>
          </div>
          {hasRecoveryItems ? (
            <div className="recovery-banner" role="status">
              {recoveryDrafts.length > 0 ? (
                <strong>Unsaved recovery drafts found</strong>
              ) : null}
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
              {unavailableRecoveryDrafts.length > 0 ? (
                <strong>Unavailable recovery data needs review</strong>
              ) : null}
              {unavailableRecoveryDrafts.map((recovery) => (
                <div
                  className="recovery-item unavailable-recovery-item"
                  key={recovery.recoveryId}
                >
                  <span>
                    {recovery.reason === "unsupported"
                      ? "Unsupported recovery format"
                      : "Corrupt recovery data"}
                    {` · ${recovery.recoveryId.slice(0, 8)}`}
                  </span>
                  <button
                    type="button"
                    onClick={() => handleExportUnavailableRecovery(recovery)}
                    disabled={busy}
                  >
                    Export data
                  </button>
                  <button
                    className="discard-recovery"
                    type="button"
                    onClick={() => handleDeleteUnavailableRecovery(recovery)}
                    disabled={busy}
                  >
                    {pendingRecoveryDeleteId === recovery.recoveryId
                      ? "Confirm delete"
                      : "Delete…"}
                  </button>
                </div>
              ))}
              {recoveryNotice ? (
                <span className="recovery-notice">{recoveryNotice}</span>
              ) : null}
            </div>
          ) : null}
          {watcherNotice ? (
            <div className="watcher-banner" role="status">
              {watcherNotice}
            </div>
          ) : null}
          {operationError ? <div className="error-banner" role="alert">{operationError}</div> : null}
          {activeNote ? (
            <Suspense
              fallback={
                <div className="markdown-editor editor-loading" role="status">
                  Loading editor…
                </div>
              }
            >
              <MarkdownEditor
                key={activeNote.relativePath}
                ariaLabel="Markdown source"
                value={draft}
                onChange={handleDraftChange}
                mode={effectiveEditorMode}
                readOnly={isMixedLineEnding}
              />
            </Suspense>
          ) : (
            <div
              aria-label="Markdown source"
              aria-multiline="true"
              aria-readonly="true"
              className="markdown-editor editor-placeholder"
              role="textbox"
            >
              Select a Markdown note from the file list.
            </div>
          )}
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
