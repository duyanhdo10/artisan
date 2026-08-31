import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { normalizeCommandError, saveNote } from "./tauri";

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
