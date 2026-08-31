import { history, redo, undo } from "@codemirror/commands";
import { commonmarkLanguage } from "@codemirror/lang-markdown";
import { Compartment, EditorState, type Transaction } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { describe, expect, it } from "vitest";
import {
  buildMarkdownPreviewDecorations,
  markdownLivePreview,
} from "./markdownPreview";

interface DecorationSnapshot {
  from: number;
  to: number;
  kind?: string;
  owner?: string;
  level?: number;
}

function snapshots(decorations: DecorationSet, length: number) {
  const result: DecorationSnapshot[] = [];
  decorations.between(0, length, (from, to, decoration) => {
    result.push({
      from,
      to,
      kind: decoration.spec.astianKind as string | undefined,
      owner: decoration.spec.astianOwner as string | undefined,
      level: decoration.spec.astianLevel as number | undefined,
    });
  });
  return result;
}

function createState(doc: string, anchor = doc.length) {
  return EditorState.create({
    doc,
    selection: { anchor },
    extensions: [commonmarkLanguage, history(), markdownLivePreview],
  });
}

describe("Markdown live preview spike", () => {
  it("decorates headings and strong emphasis without changing Markdown", () => {
    const doc = "# Heading\nA **bold** word";
    const state = createState(doc);
    const decorationSnapshot = snapshots(
      buildMarkdownPreviewDecorations(state),
      state.doc.length,
    );

    expect(state.doc.toString()).toBe(doc);
    expect(decorationSnapshot).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "heading", level: 1 }),
        expect.objectContaining({ kind: "strong" }),
        expect.objectContaining({
          kind: "hidden-syntax",
          owner: "ATXHeading1",
        }),
      ]),
    );
    expect(
      decorationSnapshot.filter(
        (item) =>
          item.kind === "hidden-syntax" && item.owner === "StrongEmphasis",
      ),
    ).toHaveLength(2);
  });

  it("reveals strong-emphasis syntax when the selection enters it", () => {
    const doc = "Outside **đậm** text";
    const boldTextPosition = doc.indexOf("đậm") + 1;
    const state = createState(doc, boldTextPosition);
    const decorationSnapshot = snapshots(
      buildMarkdownPreviewDecorations(state),
      state.doc.length,
    );

    expect(
      decorationSnapshot.some(
        (item) =>
          item.kind === "hidden-syntax" && item.owner === "StrongEmphasis",
      ),
    ).toBe(false);
    expect(decorationSnapshot).toEqual(
      expect.arrayContaining([expect.objectContaining({ kind: "strong" })]),
    );
  });

  it("keeps Unicode edits, selection, and undo/redo intact", () => {
    let state = createState("# Ghi chú\n");
    const dispatch = (transaction: Transaction) => {
      state = transaction.state;
    };

    state = state.update({
      changes: { from: state.doc.length, insert: "Tiếng Việt **ổn định**" },
      selection: { anchor: state.doc.length + "Tiếng Việt **ổn định**".length },
      userEvent: "input.type",
    }).state;

    const editedSelection = state.selection.main.head;
    expect(state.doc.toString()).toBe("# Ghi chú\nTiếng Việt **ổn định**");
    expect(editedSelection).toBe(state.doc.length);
    expect(undo({ state, dispatch })).toBe(true);
    expect(state.doc.toString()).toBe("# Ghi chú\n");
    expect(redo({ state, dispatch })).toBe(true);
    expect(state.doc.toString()).toBe("# Ghi chú\nTiếng Việt **ổn định**");
  });

  it("switches Source and Live Preview extensions without touching content", () => {
    const mode = new Compartment();
    const doc = "## Chế độ\n**Nội dung**";
    let state = EditorState.create({
      doc,
      selection: { anchor: 5 },
      extensions: [commonmarkLanguage, mode.of(markdownLivePreview)],
    });
    const selectionBefore = state.selection.main;

    state = state.update({ effects: mode.reconfigure([]) }).state;

    expect(state.doc.toString()).toBe(doc);
    expect(state.selection.main).toEqual(selectionBefore);
  });
});
