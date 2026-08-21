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

import { useUiSettings, type Theme } from "../composables/useUiSettings";
import { currentLocale } from "../i18n";
import { searchPhrases } from "../lib/editorPhrases";

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

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  /**
   * `Ctrl+S`（mac は `Cmd+S`）が押された。**保存するかは親が決める。**
   *
   * ここは合図しか出さない — 保存できるかの条件（未変更 / 保存中 / JSON が
   * 壊れている）は保存ボタンの `:disabled` が既に持っており、こちらでもう一度
   * 書くと**同じ規律が 2 箇所に生える**。親は**ボタンと同じ述語**で受けること。
   */
  (e: "save"): void;
}>();

const host = ref<HTMLElement | null>(null);
const languageCompartment = new Compartment();
const readOnlyCompartment = new Compartment();
const placeholderCompartment = new Compartment();
const themeCompartment = new Compartment();
const phraseCompartment = new Compartment();
let editor: EditorView | null = null;

const { settings } = useUiSettings();

function languageExtension(language: EditorLanguage) {
  return language === "json" ? [json(), linter(jsonParseLinter())] : markdown();
}

function readOnlyExtension(readonly: boolean) {
  return [EditorState.readOnly.of(readonly), EditorView.editable.of(!readonly)];
}

/**
 * エディタの配色。**色はすべて `style.css` のトークンを引く**ので、値そのものは
 * テーマに自動で追従する。
 *
 * ただし `dark` フラグだけは追従しない — CodeMirror はこの真偽値で
 * `&dark` / `&light` の**別系統の既定**（検索一致の強調・補完候補の選択色・
 * プレースホルダ・特殊文字）を選ぶ。ここを固定すると、その 4 つだけが
 * 反対のテーマの色で残る。`var()` で書けない値がライブラリの中にあるので、
 * **フラグを差し替える経路**（compartment）を持つ。
 */
function editorTheme(theme: Theme) {
  return EditorView.theme(THEME_SPEC, { dark: theme === "dark" });
}

const THEME_SPEC = {
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
};

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
          // **この 2 本は既定より先に置く**（同じ keymap では先頭ほど強い）。
          //
          // `Mod-` は CodeMirror の記法で **mac では Cmd に写る**。`Ctrl-` と
          // 書くと mac で効かない（サーヴァント一覧の Alt+↑↓ で「3 OS で片方だけ
          // 効かない鍵は採らない」と決めた線がそのまま当たる）。
          {
            key: "Mod-s",
            run: () => {
              emit("save");
              return true;
            },
          },
          // **リロードを飲む。** WebView の `Ctrl+R` は画面を作り直すので、
          // 編集中の本文が**確認を 1 つも通さずに消える** — dirty の確認は
          // どの面でも「閉じる」経路にしか無く、リロードはそこを通らない。
          // JetBrains 系では `Ctrl+R` が Replace なので手癖で押される
          // （この村の置換は `Ctrl+F` のパネルの中にある）。
          //
          // **エディタにフォーカスがある間だけ**塞ぐので、再読み込みで投影が
          // 張り直ることを見る手順（Spec 08 の実機確認）は今までどおり通る。
          { key: "Mod-r", run: () => true },
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
        themeCompartment.of(editorTheme(settings.theme)),
        phraseCompartment.of(EditorState.phrases.of(searchPhrases(currentLocale()))),
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

// 開いたままテーマを切り替えても追従させる（設定ダイアログと編集画面は同時に
// 開きうる）。色は `var()` が勝手に追うので、差し替えるのは `dark` フラグだけ。
watch(
  () => settings.theme,
  (theme) => editor?.dispatch({ effects: themeCompartment.reconfigure(editorTheme(theme)) }),
);

// 検索パネルの文言も表示言語へ追従させる。開いたまま言語を切り替えられるので
// （システム設定と編集画面は同時に開きうる）、テーマと同じ compartment 方式。
watch(currentLocale, (value) =>
  editor?.dispatch({
    effects: phraseCompartment.reconfigure(EditorState.phrases.of(searchPhrases(value))),
  }),
);

onBeforeUnmount(() => editor?.destroy());
</script>

<template>
  <div ref="host" class="code-editor selectable min-h-0 w-full overflow-hidden rounded border border-line" />
</template>