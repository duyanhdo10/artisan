import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearRecoveryDraft,
  listRecoveryDrafts,
  normalizeCommandError,
  readRecoveryDraft,
  saveNote,
  writeRecoveryDraft,
} from "./tauri";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
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
});
