import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearRecoveryDraft,
  createNote,
  deleteUnavailableRecoveryDraft,
  exportUnavailableRecoveryDraft,
  forgetRecentVault,
  listenVaultChanges,
  listRecentVaults,
  listRecoveryDrafts,
  normalizeCommandError,
  openRecentVault,
  readRecoveryDraft,
  reconcileVault,
  restoreLastVault,
  saveNote,
  saveNoteAsCopy,
  writeRecoveryDraft,
} from "./tauri";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
});

describe("normalizeCommandError", () => {
  it("preserves the stable Rust error envelope", () => {
    expect(
      normalizeCommandError({
        code: "invalid_note_path",
        message: "The note path is invalid.",
      }),
    ).toEqual({
      code: "invalid_note_path",
      message: "The note path is invalid.",
    });
  });

  it("normalizes transport errors without parsing their message", () => {
    expect(normalizeCommandError(new Error("IPC unavailable"))).toEqual({
      code: "ipc_error",
      message: "IPC unavailable",
    });
  });

  it("redacts unknown thrown values", () => {
    expect(normalizeCommandError({ secret: "do not expose" })).toEqual({
      code: "unknown_error",
      message: "The native Astian runtime is unavailable.",
    });
  });
});

describe("saveNote", () => {
  it("forwards relative path, editor content, and expected hash", async () => {
    vi.mocked(invoke).mockResolvedValue({
      status: "saved",
      contentHash: "b".repeat(64),
    });

    await expect(saveNote("notes/Astian.md", "# Astian\n", "a".repeat(64))).resolves.toEqual({
      status: "saved",
      contentHash: "b".repeat(64),
    });
    expect(invoke).toHaveBeenCalledWith("save_note", {
      relativePath: "notes/Astian.md",
      content: "# Astian\n",
      expectedHash: "a".repeat(64),
    });
  });
});

describe("createNote", () => {
  it("forwards only a parent-relative path and one filename segment", async () => {
    const opened = {
      relativePath: "Kế hoạch.md",
      content: "",
      contentHash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      lineEnding: "none" as const,
      hasUtf8Bom: false,
    };
    vi.mocked(invoke).mockResolvedValue(opened);

    await expect(createNote("", "Kế hoạch")).resolves.toEqual(opened);
    expect(invoke).toHaveBeenCalledWith("create_note", {
      parentRelativePath: "",
      fileName: "Kế hoạch",
    });
  });
});

describe("vault watcher IPC", () => {
  it("restores the last vault without exposing or sending an absolute path", async () => {
    const summary = { name: "Notes", notes: [], vaultSession: 3 };
    vi.mocked(invoke).mockResolvedValue(summary);

    await expect(restoreLastVault()).resolves.toEqual(summary);
    expect(invoke).toHaveBeenCalledWith("restore_last_vault");
  });

  it("requests reconciliation without sending a vault path", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(reconcileVault()).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledWith("reconcile_vault");
  });

  it("forwards only the typed watcher event payload", async () => {
    const unlisten = vi.fn();
    let nativeHandler: ((event: { payload: unknown }) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (_event, handler) => {
      nativeHandler = handler as (event: { payload: unknown }) => void;
      return unlisten;
    });
    const handler = vi.fn();
    const payload = {
      vaultSession: 2,
      revision: 4,
      status: "changed" as const,
      errorCode: null,
      changes: [],
      notes: [],
    };

    await expect(listenVaultChanges(handler)).resolves.toBe(unlisten);
    nativeHandler?.({ payload });

    expect(listen).toHaveBeenCalledWith("vault://changed", expect.any(Function));
    expect(handler).toHaveBeenCalledWith(payload);
  });
});

describe("recent vault IPC", () => {
  it("lists summaries without sending a filesystem path", async () => {
    const recent = [
      { id: "a".repeat(64), name: "Kho ghi chú", available: true },
    ];
    vi.mocked(invoke).mockResolvedValue(recent);

    await expect(listRecentVaults()).resolves.toEqual(recent);
    expect(invoke).toHaveBeenCalledWith("list_recent_vaults");
  });

  it("opens and forgets a recent vault only by opaque id", async () => {
    const id = "a".repeat(64);
    const summary = { name: "Kho ghi chú", notes: [], vaultSession: 4 };
    vi.mocked(invoke).mockResolvedValueOnce(summary).mockResolvedValueOnce(undefined);

    await expect(openRecentVault(id)).resolves.toEqual(summary);
    await expect(forgetRecentVault(id)).resolves.toBeUndefined();
    expect(invoke).toHaveBeenNthCalledWith(1, "open_recent_vault", {
      recentVaultId: id,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "forget_recent_vault", {
      recentVaultId: id,
    });
  });
});

describe("recovery draft IPC", () => {
  it("writes only the typed recovery payload", async () => {
    const summary = {
      relativePath: "notes/Astian.md",
      baseHash: "a".repeat(64),
      contentHash: "b".repeat(64),
      editorRevision: 4,
      updatedAtUnixMs: 1_788_200_000_000,
    };
    vi.mocked(invoke).mockResolvedValue(summary);

    await expect(
      writeRecoveryDraft(
        "notes/Astian.md",
        "# Bản nháp\n",
        "a".repeat(64),
        4,
        null,
      ),
    ).resolves.toEqual(summary);
    expect(invoke).toHaveBeenCalledWith("write_recovery_draft", {
      relativePath: "notes/Astian.md",
      content: "# Bản nháp\n",
      baseHash: "a".repeat(64),
      editorRevision: 4,
      expectedDraftHash: null,
    });
  });

  it("lists metadata without sending a vault path", async () => {
    vi.mocked(invoke).mockResolvedValue([]);

    await expect(listRecoveryDrafts()).resolves.toEqual([]);
    expect(invoke).toHaveBeenCalledWith("list_recovery_drafts");
  });

  it("reads and clears a selected draft by relative path", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ content: "draft" }).mockResolvedValueOnce(undefined);

    await readRecoveryDraft("notes/Astian.md");
    await clearRecoveryDraft("notes/Astian.md");

    expect(invoke).toHaveBeenNthCalledWith(1, "read_recovery_draft", {
      relativePath: "notes/Astian.md",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "clear_recovery_draft", {
      relativePath: "notes/Astian.md",
    });
  });

  it("exports and deletes unavailable data by opaque id and artifact hash", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(true).mockResolvedValueOnce(undefined);
    const recoveryId = "a".repeat(64);
    const artifactHash = "b".repeat(64);

    await expect(
      exportUnavailableRecoveryDraft(recoveryId, artifactHash),
    ).resolves.toBe(true);
    await expect(
      deleteUnavailableRecoveryDraft(recoveryId, artifactHash),
    ).resolves.toBeUndefined();

    expect(invoke).toHaveBeenNthCalledWith(
      1,
      "export_unavailable_recovery_draft",
      { recoveryId, expectedArtifactHash: artifactHash },
    );
    expect(invoke).toHaveBeenNthCalledWith(
      2,
      "delete_unavailable_recovery_draft",
      { recoveryId, expectedArtifactHash: artifactHash },
    );
  });

  it("sends recovery content to Save As Copy without an absolute path", async () => {
    vi.mocked(invoke).mockResolvedValue(null);

    await expect(
      saveNoteAsCopy("notes/missing.md", "# Recovered\n"),
    ).resolves.toBeNull();
    expect(invoke).toHaveBeenCalledWith("save_note_as_copy", {
      sourceRelativePath: "notes/missing.md",
      content: "# Recovered\n",
    });
  });
});
