import { useEffect, useRef } from "react";
import { commonmarkLanguage } from "@codemirror/lang-markdown";
import {
  Compartment,
  EditorState,
  Transaction,
  type Extension,
} from "@codemirror/state";
import { EditorView, placeholder } from "@codemirror/view";
import { minimalSetup } from "codemirror";
import { markdownLivePreview } from "./markdownPreview";

export type MarkdownEditorMode = "source" | "live-preview";

interface MarkdownEditorProps {
  value: string;
  onChange: (value: string) => void;
  mode: MarkdownEditorMode;
  disabled?: boolean;
  readOnly?: boolean;
  ariaLabel: string;
}

function editingExtension(disabled: boolean, readOnly: boolean): Extension {
  const cannotEdit = disabled || readOnly;
  return [
    EditorView.editable.of(!cannotEdit),
    EditorState.readOnly.of(cannotEdit),
  ];
}

export function MarkdownEditor({
  value,
  onChange,
  mode,
  disabled = false,
  readOnly = false,
  ariaLabel,
}: MarkdownEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  const syncingExternalValueRef = useRef(false);
  const modeCompartmentRef = useRef(new Compartment());
  const editingCompartmentRef = useRef(new Compartment());

  onChangeRef.current = onChange;

  useEffect(() => {
    if (!hostRef.current) return;

    const view = new EditorView({
      parent: hostRef.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          minimalSetup,
          commonmarkLanguage,
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({
            "aria-label": ariaLabel,
            spellcheck: "true",
          }),
          placeholder("Select a Markdown note from the file list."),
          modeCompartmentRef.current.of(
            mode === "live-preview" ? markdownLivePreview : [],
          ),
          editingCompartmentRef.current.of(
            editingExtension(disabled, readOnly),
          ),
          EditorView.updateListener.of((update) => {
            if (update.docChanged && !syncingExternalValueRef.current) {
              onChangeRef.current(update.state.doc.toString());
            }
          }),
        ],
      }),
    });

    viewRef.current = view;
    return () => {
      viewRef.current = null;
      view.destroy();
    };
    // Mount once. Value and configuration changes are synchronized below so
    // React renders do not reset CodeMirror selection or undo history.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) return;

    syncingExternalValueRef.current = true;
    try {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: value },
        annotations: Transaction.addToHistory.of(false),
      });
    } finally {
      syncingExternalValueRef.current = false;
    }
  }, [value]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: modeCompartmentRef.current.reconfigure(
        mode === "live-preview" ? markdownLivePreview : [],
      ),
    });
  }, [mode]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: editingCompartmentRef.current.reconfigure(
        editingExtension(disabled, readOnly),
      ),
    });
  }, [disabled, readOnly]);

  return <div className="markdown-editor" ref={hostRef} />;
}
