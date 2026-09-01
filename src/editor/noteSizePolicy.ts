export const LIVE_PREVIEW_SOFT_LIMIT_BYTES = 512 * 1_024;

export interface NoteSizePolicy {
  utf8Bytes: number;
  livePreviewAllowed: boolean;
}

export function getNoteSizePolicy(content: string): NoteSizePolicy {
  const utf8Bytes = new TextEncoder().encode(content).byteLength;

  return {
    utf8Bytes,
    livePreviewAllowed: utf8Bytes <= LIVE_PREVIEW_SOFT_LIMIT_BYTES,
  };
}

export function formatNoteSize(bytes: number): string {
  return `${Math.ceil(bytes / 1_024).toLocaleString("en-US")} KiB`;
}
