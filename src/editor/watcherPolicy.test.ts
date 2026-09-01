import { describe, expect, it } from "vitest";
import type { VaultChange } from "../lib/tauri";
import { decideActiveNoteWatcherAction } from "./watcherPolicy";

function change(overrides: Partial<VaultChange> = {}): VaultChange {
  return {
    kind: "modified",
    source: "external",
    relativePath: "note.md",
    previousRelativePath: null,
    contentHash: "a".repeat(64),
    ...overrides,
  };
}

describe("active note watcher policy", () => {
  it("ignores Astian writes and unrelated external changes", () => {
    expect(
      decideActiveNoteWatcherAction(
        [change({ source: "astian" })],
        "note.md",
        false,
      ),
    ).toEqual({ kind: "none" });
    expect(
      decideActiveNoteWatcherAction(
        [change({ relativePath: "other.md" })],
        "note.md",
        false,
      ),
    ).toEqual({ kind: "none" });
  });

  it("reloads clean modifications and follows a clean rename", () => {
    expect(
      decideActiveNoteWatcherAction([change()], "note.md", false),
    ).toEqual({ kind: "reload", relativePath: "note.md" });
    expect(
      decideActiveNoteWatcherAction(
        [
          change({
            kind: "renamed",
            relativePath: "renamed.md",
            previousRelativePath: "note.md",
          }),
        ],
        "note.md",
        false,
      ),
    ).toEqual({ kind: "reload", relativePath: "renamed.md" });
  });

  it("closes a clean deleted note but conflicts for every dirty external change", () => {
    expect(
      decideActiveNoteWatcherAction(
        [change({ kind: "deleted", contentHash: null })],
        "note.md",
        false,
      ),
    ).toEqual({ kind: "close" });

    for (const kind of ["modified", "deleted", "renamed"] as const) {
      expect(
        decideActiveNoteWatcherAction(
          [
            change({
              kind,
              relativePath: kind === "renamed" ? "renamed.md" : "note.md",
              previousRelativePath: kind === "renamed" ? "note.md" : null,
            }),
          ],
          "note.md",
          true,
        ),
      ).toEqual({ kind: "conflict" });
    }
  });
});
