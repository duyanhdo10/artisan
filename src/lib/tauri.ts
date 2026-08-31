import { invoke } from "@tauri-apps/api/core";

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

export function openNote(relativePath: string): Promise<OpenedNote> {
  return invoke<OpenedNote>("open_note", { relativePath });
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

export function listRecoveryDrafts(): Promise<RecoveryDraftSummary[]> {
  return invoke<RecoveryDraftSummary[]>("list_recovery_drafts");
}

export function readRecoveryDraft(relativePath: string): Promise<RecoveryDraft> {
  return invoke<RecoveryDraft>("read_recovery_draft", { relativePath });
}

export function clearRecoveryDraft(relativePath: string): Promise<void> {
  return invoke<void>("clear_recovery_draft", { relativePath });
}
