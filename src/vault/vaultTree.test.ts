import { describe, expect, it } from "vitest";
import { buildVaultTreeRows } from "./vaultTree";

describe("buildVaultTreeRows", () => {
  it("keeps empty folders and lays nested notes out depth-first", () => {
    expect(
      buildVaultTreeRows(
        [
          { relativePath: "Projects" },
          { relativePath: "Projects/Empty" },
        ],
        [
          { relativePath: "Inbox.md", title: "Inbox" },
          { relativePath: "Projects/Astian.md", title: "Astian" },
        ],
      ),
    ).toEqual([
      { kind: "folder", relativePath: "Projects", label: "Projects", depth: 0 },
      {
        kind: "folder",
        relativePath: "Projects/Empty",
        label: "Empty",
        depth: 1,
      },
      {
        kind: "note",
        relativePath: "Projects/Astian.md",
        label: "Astian",
        depth: 1,
      },
      { kind: "note", relativePath: "Inbox.md", label: "Inbox", depth: 0 },
    ]);
  });

  it("infers missing parent folders without changing relative note identity", () => {
    expect(
      buildVaultTreeRows([], [
        {
          relativePath: "Dự án/2026/Kế hoạch.md",
          title: "Kế hoạch",
        },
      ]),
    ).toEqual([
      { kind: "folder", relativePath: "Dự án", label: "Dự án", depth: 0 },
      {
        kind: "folder",
        relativePath: "Dự án/2026",
        label: "2026",
        depth: 1,
      },
      {
        kind: "note",
        relativePath: "Dự án/2026/Kế hoạch.md",
        label: "Kế hoạch",
        depth: 2,
      },
    ]);
  });
});
