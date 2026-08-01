<script setup lang="ts">
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { json, jsonParseLinter } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { linter } from "@codemirror/lint";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  placeholder,
} from "@codemirror/view";
import { onBeforeUnmount, onMounted, ref, watch } from "vue";

type EditorLanguage = "markdown" | "json";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    language: EditorLanguage;
    placeholder?: string;
    readonly?: boolean;
  }>(),
  { placeholder: "", readonly: false },
);

const emit = defineEmits<{ (e: "update:modelValue", value: string): void }>();

const host = ref<HTMLElement | null>(null);
const languageCompartment = new Compartment();
const readOnlyCompartment = new Compartment();
const placeholderCompartment = new Compartment();
let editor: EditorView | null = null;

function languageExtension(language: EditorLanguage) {
  return language === "json" ? [json(), linter(jsonParseLinter())] : markdown();
}

function readOnlyExtension(readonly: boolean) {
  return [EditorState.readOnly.of(readonly), EditorView.editable.of(!readonly)];
}

const editorTheme = EditorView.theme(
  {
    "&": {
      height: "100%",
      backgroundColor: "var(--color-surface-0)",
      color: "var(--color-ink)",
      fontSize: "12px",
    },
    ".cm-scroller": {
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
      lineHeight: "1.6",
      overflow: "auto",
    },
    ".cm-content": { padding: "10px 0", caretColor: "var(--color-accent)" },
    ".cm-line": { padding: "0 10px" },
    ".cm-gutters": {
      backgroundColor: "var(--color-surface-1)",
      color: "var(--color-ink-dim)",
      borderRight: "1px solid var(--color-line)",
    },
    ".cm-activeLine": { backgroundColor: "color-mix(in oklab, var(--color-accent) 8%, transparent)" },
    ".cm-activeLineGutter": { backgroundColor: "color-mix(in oklab, var(--color-accent) 12%, transparent)" },
    ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
      backgroundColor: "color-mix(in oklab, var(--color-accent) 35%, transparent)",
    },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--color-accent)" },
    ".cm-tooltip": {
      border: "1px solid var(--color-line)",
      backgroundColor: "var(--color-surface-1)",
    },
    ".cm-panels": { backgroundColor: "var(--color-surface-1)", color: "var(--color-ink)" },
    ".cm-textfield": { backgroundColor: "var(--color-surface-0)", color: "var(--color-ink)" },
  },
  { dark: true },
);

onMounted(() => {
  if (!host.value) return;

  editor = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: props.modelValue,
      extensions: [
        lineNumbers(),
        highlightSpecialChars(),
        history(),
        drawSelection(),
        EditorState.allowMultipleSelections.of(true),
        EditorView.lineWrapping,
        highlightActiveLineGutter(),
        highlightActiveLine(),
        search({ top: true }),
        highlightSelectionMatches(),
        autocompletion(),
        closeBrackets(),
        keymap.of([
          indentWithTab,
          ...closeBracketsKeymap,
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap,
          ...completionKeymap,
        ]),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        languageCompartment.of(languageExtension(props.language)),
        readOnlyCompartment.of(readOnlyExtension(props.readonly)),
        placeholderCompartment.of(placeholder(props.placeholder)),
        editorTheme,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            const value = update.state.doc.toString();
            if (value !== props.modelValue) emit("update:modelValue", value);
          }
        }),
      ],
    }),
  });
});

watch(
  () => props.modelValue,
  (value) => {
    if (!editor || value === editor.state.doc.toString()) return;
    editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: value } });
  },
);

watch(
  () => props.language,
  (language) => editor?.dispatch({ effects: languageCompartment.reconfigure(languageExtension(language)) }),
);

watch(
  () => props.readonly,
  (readonly) => editor?.dispatch({ effects: readOnlyCompartment.reconfigure(readOnlyExtension(readonly)) }),
);

watch(
  () => props.placeholder,
  (value) => editor?.dispatch({ effects: placeholderCompartment.reconfigure(placeholder(value)) }),
);

onBeforeUnmount(() => editor?.destroy());
</script>

<template>
  <div ref="host" class="code-editor selectable min-h-0 w-full overflow-hidden rounded border border-line" />
</template>