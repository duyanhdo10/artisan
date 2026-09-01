import { describe, expect, it } from "vitest";
import {
  LIVE_PREVIEW_SOFT_LIMIT_BYTES,
  formatNoteSize,
  getNoteSizePolicy,
} from "./noteSizePolicy";

describe("getNoteSizePolicy", () => {
  it("allows Live Preview through the exact UTF-8 soft limit", () => {
    expect(
      getNoteSizePolicy("a".repeat(LIVE_PREVIEW_SOFT_LIMIT_BYTES)),
    ).toEqual({
      utf8Bytes: LIVE_PREVIEW_SOFT_LIMIT_BYTES,
      livePreviewAllowed: true,
    });
  });

  it("falls back based on UTF-8 bytes rather than JavaScript string length", () => {
    const vietnameseContent = "ế".repeat(
      Math.floor(LIVE_PREVIEW_SOFT_LIMIT_BYTES / 3) + 1,
    );
    const policy = getNoteSizePolicy(vietnameseContent);

    expect(vietnameseContent.length).toBeLessThan(
      LIVE_PREVIEW_SOFT_LIMIT_BYTES,
    );
    expect(policy.utf8Bytes).toBeGreaterThan(LIVE_PREVIEW_SOFT_LIMIT_BYTES);
    expect(policy.livePreviewAllowed).toBe(false);
  });

  it("formats a visible size without understating partial KiB", () => {
    expect(formatNoteSize(1_025)).toBe("2 KiB");
  });
});
