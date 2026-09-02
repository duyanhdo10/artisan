import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const VAULT_CHANGED_EVENT = "vault://changed";

export interface RuntimeInfo {
  appVersion: string;
  platform: string;
  architecture: string;
}

export interface CommandError {
  code: string;
  message: string;
}

export interface NoteEntry {
  relativePath: string;
  title: string;
}

export interface VaultSummary {
  name: string;
  notes: NoteEntry[];
  vaultSession: number;
}

export interface CreatedFolder {
  relativePath: string;
}

export interface RecentVault {
  id: string;
  name: string;
  available: boolean;
}

export interface VaultChange {
  kind: "created" | "modified" | "deleted" | "renamed";
  source: "astian" | "external";
  relativePath: string;
  previousRelativePath: string | null;
  contentHash: string | null;
}

export interface VaultWatcherEvent {
  vaultSession: number;
  revision: number;
  status: "changed" | "rescan_required";
  errorCode: string | null;
  changes: VaultChange[];
  notes: NoteEntry[];
}

export interface OpenedNote {
  relativePath: string;
  content: string;
  contentHash: string;
  lineEnding: "lf" | "crlf" | "mixed" | "none";
  hasUtf8Bom: boolean;
}

export interface SaveResult {
  status: "saved" | "unchanged";
  contentHash: string;
}

export interface RecoveryDraftSummary {
  relativePath: string;
  baseHash: string;
  contentHash: string;
  editorRevision: number;
  updatedAtUnixMs: number;
}

export interface RecoveryDraft extends RecoveryDraftSummary {
  content: string;
}

export interface UnavailableRecoveryDraft {
  status: "unavailable";
  recoveryId: string;
  artifactHash: string;
  reason: "corrupt" | "unsupported";
}

export type RecoveryDraftListItem =
  | { status: "available"; draft: RecoveryDraftSummary }
  | UnavailableRecoveryDraft;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function normalizeCommandError(error: unknown): CommandError {
  if (
    isRecord(error) &&
    typeof error.code === "string" &&
    typeof error.message === "string"
  ) {
    return { code: error.code, message: error.message };
  }

  if (error instanceof Error) {
    return { code: "ipc_error", message: error.message };
  }

  return { code: "unknown_error", message: "The native Astian runtime is unavailable." };
}

export function getRuntimeInfo(): Promise<RuntimeInfo> {
  return invoke<RuntimeInfo>("get_runtime_info");
}

export function selectVault(): Promise<VaultSummary | null> {
  return invoke<VaultSummary | null>("select_vault");
}

export function restoreLastVault(): Promise<VaultSummary | null> {
  return invoke<VaultSummary | null>("restore_last_vault");
}

export function listRecentVaults(): Promise<RecentVault[]> {
  return invoke<RecentVault[]>("list_recent_vaults");
}

export function openRecentVault(recentVaultId: string): Promise<VaultSummary> {
  return invoke<VaultSummary>("open_recent_vault", { recentVaultId });
}

export function forgetRecentVault(recentVaultId: string): Promise<void> {
  return invoke<void>("forget_recent_vault", { recentVaultId });
}

export function reconcileVault(): Promise<void> {
  return invoke<void>("reconcile_vault");
}

export function listenVaultChanges(
  handler: (event: VaultWatcherEvent) => void,
): Promise<UnlistenFn> {
  return listen<VaultWatcherEvent>(VAULT_CHANGED_EVENT, (event) => {
    handler(event.payload);
  });
}

export function openNote(relativePath: string): Promise<OpenedNote> {
  return invoke<OpenedNote>("open_note", { relativePath });
}

export function restoreActiveNote(): Promise<OpenedNote | null> {
  return invoke<OpenedNote | null>("restore_active_note");
}

export function rememberActiveNote(relativePath: string | null): Promise<void> {
  return invoke<void>("remember_active_note", { relativePath });
}

export function createNote(
  parentRelativePath: string,
  fileName: string,
): Promise<OpenedNote> {
  return invoke<OpenedNote>("create_note", { parentRelativePath, fileName });
}

export function createFolder(
  parentRelativePath: string,
  folderName: string,
): Promise<CreatedFolder> {
  return invoke<CreatedFolder>("create_folder", {
    parentRelativePath,
    folderName,
  });
}

export function saveNote(
  relativePath: string,
  content: string,
  expectedHash: string,
): Promise<SaveResult> {
  return invoke<SaveResult>("save_note", {
    relativePath,
    content,
    expectedHash,
  });
}

export function writeRecoveryDraft(
  relativePath: string,
  content: string,
  baseHash: string,
  editorRevision: number,
  expectedDraftHash: string | null,
): Promise<RecoveryDraftSummary> {
  return invoke<RecoveryDraftSummary>("write_recovery_draft", {
    relativePath,
    content,
    baseHash,
    editorRevision,
    expectedDraftHash,
  });
}

export function listRecoveryDrafts(): Promise<RecoveryDraftListItem[]> {
  return invoke<RecoveryDraftListItem[]>("list_recovery_drafts");
}

export function readRecoveryDraft(relativePath: string): Promise<RecoveryDraft> {
  return invoke<RecoveryDraft>("read_recovery_draft", { relativePath });
}

export function clearRecoveryDraft(relativePath: string): Promise<void> {
  return invoke<void>("clear_recovery_draft", { relativePath });
}

export function exportUnavailableRecoveryDraft(
  recoveryId: string,
  expectedArtifactHash: string,
): Promise<boolean> {
  return invoke<boolean>("export_unavailable_recovery_draft", {
    recoveryId,
    expectedArtifactHash,
  });
}

export function deleteUnavailableRecoveryDraft(
  recoveryId: string,
  expectedArtifactHash: string,
): Promise<void> {
  return invoke<void>("delete_unavailable_recovery_draft", {
    recoveryId,
    expectedArtifactHash,
  });
}

export function saveNoteAsCopy(
  sourceRelativePath: string,
  content: string,
): Promise<OpenedNote | null> {
  return invoke<OpenedNote | null>("save_note_as_copy", {
    sourceRelativePath,
    content,
  });
}
