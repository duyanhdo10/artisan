import type { AutosaveSaveState } from "./autosavePolicy";

export interface WindowCloseContext {
  isDirty: boolean;
  isBusy: boolean;
  hasMixedLineEndings: boolean;
  saveState: AutosaveSaveState;
}

export type WindowCloseDecision =
  | { kind: "allow" }
  | { kind: "block"; message: string }
  | { kind: "flush"; shouldStartSave: boolean; message: string };

export function decideWindowClose(
  context: WindowCloseContext,
): WindowCloseDecision {
  if (
    context.isDirty &&
    (context.isBusy ||
      context.hasMixedLineEndings ||
      context.saveState === "conflict")
  ) {
    return {
      kind: "block",
      message:
        context.saveState === "conflict"
          ? "Resolve the external conflict before closing Astian."
          : "The modified note could not be flushed before closing Astian.",
    };
  }

  if (
    context.isDirty ||
    context.saveState === "saving" ||
    context.saveState === "queued"
  ) {
    return {
      kind: "flush",
      shouldStartSave: context.saveState !== "saving",
      message: "Saving the modified note before closing Astian…",
    };
  }

  return { kind: "allow" };
}
