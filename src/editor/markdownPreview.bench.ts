import { commonmarkLanguage } from "@codemirror/lang-markdown";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { bench, describe } from "vitest";
import { buildMarkdownPreviewDecorations } from "./markdownPreview";

const block = [
  "## Tiêu đề hiệu năng",
  "Đây là nội dung tiếng Việt có **định dạng đậm** để đo decoration.",
  "Một dòng source Markdown bình thường để mô phỏng ghi chú dài.",
  "",
].join("\n");

const blockCount = 5_000;
const largeNote = block.repeat(blockCount);
const boldTextPosition = largeNote.indexOf("định dạng đậm") + 1;

function createSourceState() {
  return EditorState.create({
    doc: largeNote,
    selection: { anchor: largeNote.length },
    extensions: [commonmarkLanguage],
  });
}

function parseToEnd(state: EditorState) {
  const tree = ensureSyntaxTree(state, state.doc.length, 5_000);
  if (!tree || tree.length !== state.doc.length) {
    throw new Error("CommonMark parser did not reach the end of the fixture");
  }
  return tree;
}

function countDecorations(
  state: EditorState,
  tree: NonNullable<ReturnType<typeof ensureSyntaxTree>>,
) {
  let count = 0;
  buildMarkdownPreviewDecorations(state, tree).between(
    0,
    state.doc.length,
    () => {
      count += 1;
    },
  );
  return count;
}

const parsedLargeState = createSourceState();
const parsedLargeTree = parseToEnd(parsedLargeState);

const expectedDecorationCount = blockCount * 5;
const actualDecorationCount = countDecorations(parsedLargeState, parsedLargeTree);
if (actualDecorationCount !== expectedDecorationCount) {
  throw new Error(
    `Benchmark fixture produced ${actualDecorationCount} decorations; expected ${expectedDecorationCount}`,
  );
}

describe(`Markdown Live Preview (${blockCount.toLocaleString("en-US")} blocks, ${Math.round(largeNote.length / 1_024)} KiB)`, () => {
  bench("parse to EOF and build all decorations", () => {
    const state = createSourceState();
    const tree = parseToEnd(state);
    if (countDecorations(state, tree) !== expectedDecorationCount) {
      throw new Error("Invalid decoration count");
    }
  });

  bench("edit at document end and rebuild all decorations", () => {
    const state = parsedLargeState.update({
      changes: { from: parsedLargeState.doc.length, insert: "x" },
      userEvent: "input.type",
    }).state;
    const tree = parseToEnd(state);
    if (countDecorations(state, tree) !== expectedDecorationCount) {
      throw new Error("Invalid decoration count after edit");
    }
  });

  bench("move selection and rebuild all decorations", () => {
    const state = parsedLargeState.update({
      selection: { anchor: boldTextPosition },
      userEvent: "select.pointer",
    }).state;
    if (countDecorations(state, parsedLargeTree) !== expectedDecorationCount - 2) {
      throw new Error("Strong-emphasis markers were not revealed");
    }
  });
});
