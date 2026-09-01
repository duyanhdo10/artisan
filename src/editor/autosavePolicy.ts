export const AUTOSAVE_DEBOUNCE_MS = 700;

export type AutosaveSaveState =
  | "idle"
  | "queued"
  | "saving"
  | "failed"
  | "conflict";

interface AutosaveContext {
  hasActiveNote: boolean;
  isDirty: boolean;
  isBusy: boolean;
  hasMixedLineEndings: boolean;
  saveState: AutosaveSaveState;
}

export function shouldQueueAutosave({
  hasActiveNote,
  isDirty,
  isBusy,
  hasMixedLineEndings,
  saveState,
}: AutosaveContext): boolean {
  return (
    hasActiveNote &&
    isDirty &&
    !isBusy &&
    !hasMixedLineEndings &&
    (saveState === "idle" || saveState === "queued")
  );
}

export async function saveAfterRecoveryQueue(
  recoveryQueue: Promise<void>,
  isCancelled: () => boolean,
  save: () => Promise<boolean>,
): Promise<boolean> {
  await recoveryQueue;
  if (isCancelled()) return false;
  return save();
}
