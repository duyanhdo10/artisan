import { describe, expect, it } from "vitest";
import {
  AUTOSAVE_DEBOUNCE_MS,
  saveAfterRecoveryQueue,
  shouldQueueAutosave,
  type AutosaveSaveState,
} from "./autosavePolicy";

function context(saveState: AutosaveSaveState = "idle") {
  return {
    hasActiveNote: true,
    isDirty: true,
    isBusy: false,
    hasMixedLineEndings: false,
    saveState,
  };
}

describe("autosave policy", () => {
  it("uses the initial 700 ms debounce and queues a normal dirty note", () => {
    expect(AUTOSAVE_DEBOUNCE_MS).toBe(700);
    expect(shouldQueueAutosave(context())).toBe(true);
    expect(shouldQueueAutosave(context("queued"))).toBe(true);
  });

  it("waits for durable recovery work before starting save", async () => {
    let releaseRecovery: (() => void) | undefined;
    const recoveryQueue = new Promise<void>((resolve) => {
      releaseRecovery = resolve;
    });
    let saveCalled = false;
    const operation = saveAfterRecoveryQueue(
      recoveryQueue,
      () => false,
      async () => {
        saveCalled = true;
        return true;
      },
    );

    await Promise.resolve();
    expect(saveCalled).toBe(false);
    releaseRecovery?.();
    await expect(operation).resolves.toBe(true);
    expect(saveCalled).toBe(true);
  });

  it("does not save a revision whose debounce was cancelled", async () => {
    const save = () => Promise.resolve(true);
    await expect(
      saveAfterRecoveryQueue(Promise.resolve(), () => true, save),
    ).resolves.toBe(false);
  });

  it("does not create retry loops for failed, saving, or conflict states", () => {
    expect(shouldQueueAutosave(context("saving"))).toBe(false);
    expect(shouldQueueAutosave(context("failed"))).toBe(false);
    expect(shouldQueueAutosave(context("conflict"))).toBe(false);
  });

  it("refuses autosave without an editable dirty note", () => {
    expect(
      shouldQueueAutosave({ ...context(), hasActiveNote: false }),
    ).toBe(false);
    expect(shouldQueueAutosave({ ...context(), isDirty: false })).toBe(false);
    expect(shouldQueueAutosave({ ...context(), isBusy: true })).toBe(false);
    expect(
      shouldQueueAutosave({ ...context(), hasMixedLineEndings: true }),
    ).toBe(false);
  });
});
