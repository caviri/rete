// Entry point bundled (esbuild, IIFE) into ../cm6.bundle.js and inlined into the
// playground by scripts/build_playground.py. Exposes the CodeMirror 6 API the
// editor component (editor.js) needs as a single global `window.CM`.
import { EditorState, StateField, StateEffect, RangeSet, RangeSetBuilder, Compartment } from "@codemirror/state";
import { EditorView, Decoration, WidgetType, ViewPlugin, keymap, lineNumbers, drawSelection, highlightActiveLine, highlightActiveLineGutter, placeholder } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { syntaxHighlighting, HighlightStyle, StreamLanguage, bracketMatching, indentOnInput, indentUnit } from "@codemirror/language";
import { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { sparql } from "@codemirror/legacy-modes/mode/sparql";
import { turtle } from "@codemirror/legacy-modes/mode/turtle";
import { tags } from "@lezer/highlight";

window.CM = {
  EditorState, StateField, StateEffect, RangeSet, RangeSetBuilder, Compartment,
  EditorView, Decoration, WidgetType, ViewPlugin, keymap, lineNumbers, drawSelection,
  highlightActiveLine, highlightActiveLineGutter, placeholder,
  defaultKeymap, history, historyKeymap, indentWithTab,
  syntaxHighlighting, HighlightStyle, StreamLanguage, bracketMatching, indentOnInput, indentUnit,
  autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap,
  sparql, turtle, tags
};
