import { syntaxTree } from "@codemirror/language";
import {
  type EditorState,
  type Range,
  StateField,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
} from "@codemirror/view";

const headingPattern = /^ATXHeading([1-6])$/;

function selectionTouches(state: EditorState, from: number, to: number) {
  return state.selection.ranges.some(
    (range) => range.from <= to && range.to >= from,
  );
}

export function buildMarkdownPreviewDecorations(
  state: EditorState,
): DecorationSet {
  const ranges: Range<Decoration>[] = [];

  syntaxTree(state).iterate({
    enter(node) {
      const headingMatch = headingPattern.exec(node.name);
      if (headingMatch) {
        const level = Number(headingMatch[1]);
        const line = state.doc.lineAt(node.from);
        ranges.push(
          Decoration.line({
            class: `cm-astian-heading cm-astian-heading-${level}`,
            astianKind: "heading",
            astianLevel: level,
          }).range(line.from),
        );
      }

      if (node.name === "StrongEmphasis") {
        ranges.push(
          Decoration.mark({
            class: "cm-astian-strong",
            astianKind: "strong",
          }).range(node.from, node.to),
        );
      }

      if (node.name !== "HeaderMark" && node.name !== "EmphasisMark") {
        return;
      }

      const owner = node.node.parent;
      if (!owner || selectionTouches(state, owner.from, owner.to)) {
        return;
      }

      ranges.push(
        Decoration.replace({
          astianKind: "hidden-syntax",
          astianOwner: owner.name,
        }).range(node.from, node.to),
      );
    },
  });

  return Decoration.set(ranges, true);
}

const markdownPreviewField = StateField.define<DecorationSet>({
  create: buildMarkdownPreviewDecorations,
  update(value, transaction) {
    const treeChanged =
      syntaxTree(transaction.startState) !== syntaxTree(transaction.state);

    if (transaction.docChanged || transaction.selection || treeChanged) {
      return buildMarkdownPreviewDecorations(transaction.state);
    }

    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});

export const markdownLivePreview = [markdownPreviewField];
