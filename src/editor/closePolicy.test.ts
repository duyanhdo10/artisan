import { describe, expect, it } from "vitest";
import {
  decideWindowClose,
  type WindowCloseContext,
} from "./closePolicy";

function context(
  overrides: Partial<WindowCloseContext> = {},
): WindowCloseContext {
  return {
    isDirty: false,
    isBusy: false,
    hasMixedLineEndings: false,
    saveState: "idle",
    ...overrides,
  };
}

describe("window close policy", () => {
  it("allows a clean idle window to close without interception", () => {
    expect(decideWindowClose(context())).toEqual({ kind: "allow" });
  });

  it("blocks dirty conflict, busy, and mixed-line-ending notes", () => {
    expect(
      decideWindowClose(context({ isDirty: true, saveState: "conflict" })),
    ).toEqual({
      kind: "block",
      message: "Resolve the external conflict before closing Astian.",
    });

    for (const unsafe of [
      context({ isDirty: true, isBusy: true }),
      context({ isDirty: true, hasMixedLineEndings: true }),
    ]) {
      expect(decideWindowClose(unsafe)).toEqual({
        kind: "block",
        message: "The modified note could not be flushed before closing Astian.",
      });
    }
  });

  it("starts a save for dirty idle, queued, or failed notes", () => {
    for (const saveState of ["idle", "queued", "failed"] as const) {
      expect(decideWindowClose(context({ isDirty: true, saveState }))).toEqual({
        kind: "flush",
        shouldStartSave: true,
        message: "Saving the modified note before closing Astian…",
      });
    }
  });

  it("waits for a save already in progress without starting a second save", () => {
    expect(
      decideWindowClose(context({ isDirty: true, saveState: "saving" })),
    ).toEqual({
      kind: "flush",
      shouldStartSave: false,
      message: "Saving the modified note before closing Astian…",
    });
  });

  it("waits through transient clean saving or queued states", () => {
    for (const saveState of ["saving", "queued"] as const) {
      expect(decideWindowClose(context({ saveState })).kind).toBe("flush");
    }
  });
});
