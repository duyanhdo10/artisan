import type { VaultChange } from "../lib/tauri";

export type ActiveNoteWatcherAction =
  | { kind: "none" }
  | { kind: "reload"; relativePath: string }
  | { kind: "close" }
  | { kind: "conflict" };

export function decideActiveNoteWatcherAction(
  changes: VaultChange[],
  activeRelativePath: string | null,
  isDirty: boolean,
): ActiveNoteWatcherAction {
  if (!activeRelativePath) return { kind: "none" };

  const relevant = changes.find(
    (change) =>
      change.source === "external" &&
      (change.relativePath === activeRelativePath ||
        change.previousRelativePath === activeRelativePath),
  );
  if (!relevant) return { kind: "none" };
  if (isDirty) return { kind: "conflict" };

  switch (relevant.kind) {
    case "deleted":
      return { kind: "close" };
    case "created":
    case "modified":
    case "renamed":
      return { kind: "reload", relativePath: relevant.relativePath };
  }
}
