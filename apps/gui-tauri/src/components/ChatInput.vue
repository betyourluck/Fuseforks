<script setup lang="ts">
/**
 * 会話の入力欄。
 *
 * 作りは Kataribe の `ActionInput.vue` に倣う:
 * - `rows="1"` から始め、内容に合わせて**上方向へ伸びる**（下端固定のレイアウトなので）
 * - 上限まで伸びたら内部スクロールへ切り替える
 * - 送信ボタンは入力欄の**中**に浮かせ、中身があるときだけ現れる
 * - Enter で送信、Shift+Enter で改行
 *
 * 高さを CSS だけで賄えないのは、`textarea` に内容ぴったりへ縮む仕組みが無いため。
 * `height: auto` へ戻してから `scrollHeight` を読む、の 2 段が要る。
 *
 * 画像の添付（Spec 23 P4）。入口は**貼り付けと「参照…」の 2 つ**（D4 —
 * ドラッグ&ドロップは Tauri の `dragDropEnabled` が drop を横取りするので
 * 入れない）。変換（縮小 + WebP 化）は WebWorker で行い、この画面は
 * チップの表示と送信への同乗だけを持つ。上限は 1 発話 1 枚（D5）で、
 * 2 枚目を選んだら**置き換える**（チップが 1 枚しか出ない画面で「2 枚目は
 * 拒否」にすると、置き換えたつもりの操作が黙って無視される）。
 *
 * 会話の参照（Spec 58）。`@@` でこの会話のサーヴァントの発話を選ぶと、参照の
 * チップが付く。**入力欄の文面には何も挿さない** — 運ぶのは発話 ID だけで、写しを
 * 作るのはコア。3 件まで。
 */
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import {
  AttachmentError,
  MAX_EDGE_PX,
  prepareFile,
  type PendingAttachment,
} from "../lib/attachment";
import { carries, carriersOf, effectiveProvider } from "../lib/carries";
import {
  applyCompletion,
  findTrigger,
  rankCandidates,
  removeTrigger,
  splitForDisplay,
  type Candidate,
  type Trigger,
} from "../lib/pathComplete";
import {
  MAX_QUOTES,
  addChip,
  chipOf,
  quoteCandidates,
  rankQuotes,
  restoreDraft,
  shortTime,
  type QuoteCandidate,
  type QuoteChip,
} from "../lib/quoteRef";
import { listWorkDirFiles } from "../lib/ipc";
import { useOrchestrator } from "../composables/useOrchestrator";
import { contextArc, contextPercent, contextRatio, contextTone } from "../lib/contextUsage";
import type { AgentId } from "../types";

const props = defineProps<{
  /** 送信できない状態か。 */
  disabled: boolean;
  /** 入力欄のプレースホルダ。 */
  placeholder: string;
  /** 送信できないときの理由。ボタンの `title` に出す。 */
  blockedReason?: string;
  /** 宛先のサーヴァント。パス補完の候補源を決める（Spec 24 D3）。 */
  agentId?: AgentId | null;
  /**
   * 宛先の作業フォルダ。`null` なら候補を出さず理由を出す。
   *
   * **判定をここでするのは、フロントが既にこの情報を持っているから**
   * （`AgentSnapshot.workDir`）。未設定なら IPC を呼ばずに済む（D3）。
   */
  workDir?: string | null;
  /**
   * 表示クリアを押せるか（出す行が 1 つでもあるか）。
   *
   * **判定はここでしない** — 何が見えているかを知っているのは会話ペインの側で、
   * 入力欄は「押されたことを伝える」だけ。ここで数えると同じ規律が 2 箇所に生える。
   */
  canClear?: boolean;
  /**
   * 宛先のテンプレートの `contextLength`（Spec 49 の輪の分母）。`null` なら輪を出さない。
   * `workDir` と同じで、判定に要る材料はフロントが既に持っている（`state.templates`）
   * ので prop で受け、ここでは IPC を呼ばない。
   */
  contextLength?: number | null;
  /** 宛先の直近の呼び出しの入力（`AgentSnapshot.lastPromptTokens`。輪の分子）。 */
  lastPromptTokens?: number | null;
  /**
   * 送信の実体。**成否を返す**（Spec 58 D3）。
   *
   * イベント（`emit`）ではなく関数で受けるのは、入力欄が結果を知る必要があるから —
   * 下書きは送る前に消すので、拒否されたら文面・添付・参照を戻す。`emit` は親の
   * 戻り値を受け取れない。
   */
  submit: (
    text: string,
    attachments: PendingAttachment[],
    quoteIds: string[],
  ) => Promise<boolean>;
}>();

/**
 * コンテキスト使用率の輪（Spec 49）。**分子は直近の 1 呼び出し**であって
 * ターンの合計ではない — 合計だと 6 周のターンで窓の何倍にもなる（D1 の罠）。
 * `null` = 出さない（未選択 / テンプレート無し / まだ呼び出していない）。
 */
const contextUsage = computed(() => {
  const ratio = contextRatio(props.lastPromptTokens, props.contextLength);
  if (ratio === null) return null;
  return {
    ratio,
    percent: contextPercent(ratio),
    arc: contextArc(ratio),
    tone: contextTone(ratio),
    over: ratio > 1,
  };
});

/** 輪の周長（r = 5.5）。`stroke-dasharray` の分母。 */
const RING_CIRCUMFERENCE = 2 * Math.PI * 5.5;

const emit = defineEmits<{
  /** 表示クリア。**中身は消さない**ので、実際に隠すのは会話ペインの側。 */
  (e: "clearView"): void;
}>();

const { t } = useI18n();
const orchestrator = useOrchestrator();

/** これを超えたら伸びるのをやめて内部スクロールにする。 */
const MAX_HEIGHT_PX = 220;

const text = ref("");
const area = ref<HTMLTextAreaElement | null>(null);
const filePicker = ref<HTMLInputElement | null>(null);

/** 送信待ちの添付（D5: 1 枚まで）。 */
const attachment = ref<PendingAttachment | null>(null);
/** 変換中か。中は送信もチップの × も待たせる。 */
const converting = ref(false);
/** 送信待ちの会話の参照（Spec 58。3 件まで）。 */
const quotes = ref<readonly QuoteChip[]>([]);
/** 参照が上限に達しているか。候補の行を押せなくし、理由を出す。 */
const quotesFull = computed(() => quotes.value.length >= MAX_QUOTES);

const canSend = computed(
  () =>
    (!!text.value.trim() || !!attachment.value || quotes.value.length > 0) &&
    !props.disabled &&
    !converting.value,
);

/** チップに出すサイズ表記。 */
const attachmentSize = computed(() => {
  if (!attachment.value) return "";
  return `${Math.max(1, Math.round(attachment.value.bytes / 1024))} KB`;
});

/** チップに出す寸法（画像のときだけ）。 */
const attachmentDimensions = computed(() => {
  const a = attachment.value;
  if (!a || a.width === undefined || a.height === undefined) return "";
  return `${a.width}×${a.height} / `;
});

/**
 * 宛先のワイヤ（Spec 36 D12）。テンプレート未設定なら base URL から推定する
 * （コアの `effective_provider` と同じ規則）。
 */
const targetProvider = computed(() => {
  const agent = orchestrator.state.agents.find((a) => a.id === props.agentId);
  if (!agent) return null;
  const template = orchestrator.state.templates.find(
    (t) => t.id === agent.modelTemplateId,
  );
  if (!template) return null;
  return effectiveProvider(template.provider, template.baseUrl);
});

/**
 * 貼った添付を宛先が運べないときの警告（Spec 36 D12）。
 *
 * **送信は止めない** — 止めるのはコア側の門で、ここは「貼った時点で分かる」
 * ためだけ。2 箇所で止めると、表の同期が切れたときにどちらが正か読めなくなる。
 */
const carryWarning = computed(() => {
  const a = attachment.value;
  const provider = targetProvider.value;
  if (!a || !provider || carries(provider, a.kind)) return "";
  return t("chatInput.attachNotCarried", {
    kind: t(`chatInput.attachKind.${a.kind}`),
    carriers: carriersOf(a.kind).join(" / "),
  });
});

// ---- パス補完（Spec 24 P2） -------------------------------------------------

/** 検出中の `@` クエリ。`null` なら補完は閉じている。 */
const trigger = ref<Trigger | null>(null);
/** 候補の全件。`@` を開いた瞬間に 1 回だけ取る（D6）。 */
const candidates = ref<Candidate[]>([]);
/** 走査が上限で打ち切られたか（D4。真なら画面に出す）。 */
const candidatesTruncated = ref(false);
/** 候補の取得中。 */
const loadingCandidates = ref(false);
/** 候補の取得に失敗したか。**トーストは出さない** — 打鍵のたびに鳴る。 */
const candidatesFailed = ref(false);
/** 反転表示している候補の位置。 */
const selectedIndex = ref(0);

/** 作業フォルダが無い個体では候補を出さない（D3）。 */
const hasWorkDir = computed(() => !!props.workDir);

/**
 * 表示する候補（順位付け済み + 表示用の分割）。
 *
 * **`base` と `dir` を分けて持つ** — 照合が basename 優先なら表示もそうで
 * なければ、打っている対象が行の中で埋もれる（D5）。
 */
const suggestions = computed(() =>
  (trigger.value && hasWorkDir.value
    ? rankCandidates(candidates.value, trigger.value.query)
    : []
  ).map((item) => ({ ...item, ...splitForDisplay(item.candidate.id) })),
);

/**
 * 補完のポップアップが**キー操作を奪う**状態か。
 *
 * **候補が 1 件も無いときは奪わない** — 一致しないクエリを打っている最中に
 * Enter が飲まれると、送信できない理由が画面から読めなくなる。
 * 「作業フォルダが無い」「打ち切り」の注記だけを出しているときも同じ。
 */
const popupCaptures = computed(
  () =>
    !!trigger.value &&
    (isQuoteTrigger.value ? quoteSuggestions.value.length > 0 : suggestions.value.length > 0),
);

/** 枠自体を出すか（候補ゼロでも理由を出すことがある）。 */
const popupVisible = computed(() => {
  if (!trigger.value) return false;
  // 参照: 候補が出ているか、**この会話にサーヴァントの発話がまだ 1 つも無い**とき。
  // 後者で黙ると「`@@` が壊れている」と読まれる。一致しないクエリでは閉じる
  // （ファイルと同じ — Enter を奪わない）。
  if (isQuoteTrigger.value) {
    return quoteSuggestions.value.length > 0 || quotePool.value.length === 0;
  }
  return (
    suggestions.value.length > 0 ||
    !hasWorkDir.value ||
    loadingCandidates.value ||
    candidatesFailed.value
  );
});

// ---- 会話の参照（Spec 58） ---------------------------------------------------

/** いま開いている補完が会話の参照（`@@`）か。 */
const isQuoteTrigger = computed(() => trigger.value?.kind === "message");

/**
 * 参照の候補の全件。**`@@` を開いた瞬間に 1 回だけ組む**（ファイルの D6 と同じ理由 —
 * 打鍵の途中で新しい発話が届いて並びが入れ替わると、絞り込みの結果が揺れる）。
 * 元は `state.messages` なので IPC は要らない。
 */
const quotePool = ref<QuoteCandidate[]>([]);

/** 表示する参照の候補（順位付け済み）。 */
const quoteSuggestions = computed(() =>
  trigger.value && isQuoteTrigger.value ? rankQuotes(quotePool.value, trigger.value.query) : [],
);

// 会話を切り替えたら、付けていた参照は外す。参照できるのは**この会話の**発話だけで、
// 残しておくとコアに必ず拒まれるチップが入力欄に居座る。
watch(
  () => orchestrator.state.currentSessionId,
  () => {
    quotes.value = [];
  },
);

/** 宛先の表示（利用者 / サーヴァントの表示名 / 外部）。 */
function quoteTarget(candidate: QuoteCandidate): string {
  const to = candidate.to;
  if (to.kind === "agent") {
    return orchestrator.state.agents.find((a) => a.id === to.id)?.name ?? to.id;
  }
  return t(`chatInput.quoteTo.${to.kind}`);
}

/** カーソル位置から `@` を検出し直す。 */
function refreshTrigger(): void {
  const el = area.value;
  if (!el || props.disabled) {
    trigger.value = null;
    return;
  }
  trigger.value = findTrigger(text.value, el.selectionStart ?? 0);
}

/** 補完を閉じ、取った候補も捨てる。 */
function closeCompletion(): void {
  trigger.value = null;
  candidates.value = [];
  candidatesTruncated.value = false;
  candidatesFailed.value = false;
}

// 開いた瞬間に 1 回だけ取る（D6）。**開いている間は取り直さない** —
// 一覧が打鍵の途中で入れ替わると、絞り込みの結果が揺れる。
//
// **見るのは「開いているか」ではなく種別**（Spec 58）— `@` を打った直後に `@` を
// もう 1 つ打つと、補完は開いたままファイルから会話の参照へ入れ替わる。
watch(
  () => trigger.value?.kind ?? null,
  async (kind) => {
    candidates.value = [];
    candidatesTruncated.value = false;
    candidatesFailed.value = false;
    quotePool.value = [];
    if (kind === null) return;
    selectedIndex.value = 0;
    if (kind === "message") {
      quotePool.value = quoteCandidates(
        orchestrator.state.messages,
        (id) => orchestrator.state.agents.find((a) => a.id === id)?.name ?? null,
      );
      return;
    }
    // 作業フォルダが無いなら呼ばない（判定に必要な情報を既に持っている）。
    if (!props.agentId || !hasWorkDir.value) return;
    loadingCandidates.value = true;
    candidatesFailed.value = false;
    try {
      const listing = await listWorkDirFiles(props.agentId);
      candidates.value = listing.paths.map((id) => ({ id, kind: "file" as const }));
      candidatesTruncated.value = listing.truncated;
    } catch {
      // 生の ipc を直に呼んでいるので、失敗はここで受ける。
      // **トーストにしない** — `@` は打鍵のたびに開きうる。
      candidatesFailed.value = true;
    } finally {
      loadingCandidates.value = false;
    }
  },
);

// 絞り込みで件数が減ったとき、反転が範囲外に残らないようにする。
watch([suggestions, quoteSuggestions], () => {
  if (selectedIndex.value >= activeCount.value) selectedIndex.value = 0;
});

/** いま出している候補の件数（種別を問わない）。 */
const activeCount = computed(() =>
  isQuoteTrigger.value ? quoteSuggestions.value.length : suggestions.value.length,
);

/**
 * 参照の候補を確定する。**本文へは何も挿さない** — 打ちかけの `@@クエリ` を消して
 * チップを足すだけ（Spec 58 D3）。上限に達していたら何もしない（候補の行は
 * 押せない見た目にしてあり、理由は枠の下に出ている）。
 */
async function confirmQuote(candidate: QuoteCandidate): Promise<void> {
  const el = area.value;
  const current = trigger.value;
  if (!el || !current || quotesFull.value) return;
  quotes.value = addChip(quotes.value, chipOf(candidate));
  const result = removeTrigger(text.value, current, el.selectionStart ?? text.value.length);
  text.value = result.text;
  closeCompletion();
  await nextTick();
  autoGrow();
  el.focus();
  el.setSelectionRange(result.caret, result.caret);
}

/** 反転している候補を確定する（種別で分ける）。 */
function confirmSelected(): void {
  if (isQuoteTrigger.value) {
    void confirmQuote(quoteSuggestions.value[selectedIndex.value]);
  } else {
    void confirmCandidate(suggestions.value[selectedIndex.value].candidate);
  }
}

/** 候補を確定して本文へ挿す。 */
async function confirmCandidate(candidate: Candidate): Promise<void> {
  const el = area.value;
  const current = trigger.value;
  if (!el || !current) return;
  const result = applyCompletion(
    text.value,
    current,
    el.selectionStart ?? text.value.length,
    candidate,
  );
  text.value = result.text;
  closeCompletion();
  await nextTick();
  autoGrow();
  el.focus();
  el.setSelectionRange(result.caret, result.caret);
}

/** ↑↓ で選択を動かす。端では巻き戻す。 */
function moveSelection(delta: number, event: KeyboardEvent): void {
  if (!popupCaptures.value) return;
  event.preventDefault();
  const count = activeCount.value;
  selectedIndex.value = (selectedIndex.value + delta + count) % count;
}

/** Esc で閉じる。開いていなければ何もしない（他の Esc の邪魔をしない）。 */
function onEscape(event: KeyboardEvent): void {
  if (!trigger.value) return;
  event.preventDefault();
  closeCompletion();
}

/** Tab で確定（VS Code と同じ作法）。 */
function onTab(event: KeyboardEvent): void {
  if (!popupCaptures.value) return;
  event.preventDefault();
  confirmSelected();
}

/** 内容ぴったりの高さへ合わせる。 */
function autoGrow(): void {
  const el = area.value;
  if (!el) return;
  // 一度 auto へ戻さないと scrollHeight が縮まず、削っても高さが残る。
  el.style.height = "auto";
  el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT_PX)}px`;
}

/**
 * 送信。**下書きは送る前に消し、拒否されたら戻す**（Spec 58 D3）。
 *
 * 先に消すのは二重送信を防ぐためで（Enter の連打）、この順は変えない。代わりに
 * 結果を待ち、失敗したら文面・添付・参照の 3 つを [`restoreDraft`] の 1 実装で戻す —
 * コアが参照を拒む経路（`INVALID_QUOTE`）ができたので、拒まれたときに書いた依頼文ごと
 * 失わせない。待っている間に打たれたものは上書きしない。
 */
async function send(): Promise<void> {
  if (!canSend.value) return;
  const failed = { text: text.value, attachment: attachment.value, quotes: quotes.value };
  text.value = "";
  attachment.value = null;
  quotes.value = [];
  closeCompletion();
  // 空にした後で高さを最小へ戻す。
  await nextTick();
  autoGrow();

  const ok = await props.submit(
    failed.text,
    failed.attachment ? [failed.attachment] : [],
    failed.quotes.map((chip) => chip.id),
  );
  if (ok) return;

  const restored = restoreDraft(
    { text: text.value, attachment: attachment.value, quotes: quotes.value },
    failed,
  );
  text.value = restored.text;
  attachment.value = restored.attachment;
  quotes.value = restored.quotes;
  await nextTick();
  autoGrow();
}

/**
 * Enter の扱い。**意味が 3 つあるので、順序を仕様として固定する**
 * （Spec 24 P2 の表）。
 *
 * | 順 | 条件 | 意味 |
 * |---|---|---|
 * | 1 | `event.isComposing` | IME の変換確定 |
 * | 2 | 補完が候補を出している | 候補の確定（送信しない） |
 * | 3 | それ以外 | 送信 |
 *
 * **1 が最優先なのがこの村に固有の点。** 日本語入力では「変換確定の Enter」が
 * 最も頻度が高く、ここを 2 番目に置くと**日本語でだけ壊れる** —
 * 変換を確定した瞬間に候補が入るか、未完成の文が飛ぶ。
 * 型検査にも既存のテストにも掛からない種類なので、順序をここへ書いておく。
 *
 * 補完を開いたまま送りたいときは `Esc` → `Enter` の 2 打鍵（VS Code と同じ）。
 */
function onEnter(event: KeyboardEvent): void {
  if (event.isComposing) return;
  if (popupCaptures.value) {
    event.preventDefault();
    confirmSelected();
    return;
  }
  event.preventDefault();
  void send();
}

/** 失敗の種別 → 辞書キー。生の例外文字列は画面に出さない。 */
const ATTACH_ERROR_KEYS = {
  tooLarge: "chatInput.attachTooLarge",
  convertedTooLarge: "chatInput.attachConvertedTooLarge",
  convertFailed: "chatInput.attachFailed",
  unsupportedType: "chatInput.attachUnsupported",
} as const;

/** 変換の失敗を辞書の文言で通知する。 */
function notifyAttachError(error: unknown): void {
  const kind = error instanceof AttachmentError ? error.kind : "convertFailed";
  orchestrator.notify("error", t(ATTACH_ERROR_KEYS[kind]));
}

/** ファイルを 1 件受け取り、（画像なら変換して）チップに載せる。 */
async function attach(file: File): Promise<void> {
  if (props.disabled || converting.value) return;
  converting.value = true;
  try {
    attachment.value = await prepareFile(file);
  } catch (error) {
    notifyAttachError(error);
  } finally {
    converting.value = false;
  }
}

/**
 * 貼り付け。クリップボードに画像があるときだけ横取りする —
 * テキストの貼り付けは textarea の既定動作のまま。
 */
function onPaste(event: ClipboardEvent): void {
  const items = event.clipboardData?.items ?? [];
  for (const item of items) {
    // **種別の判定は中身（magic）が決める**ので、ここは「ファイルが来たか」
    // だけを見る。MIME で絞ると、クリップボードが型を伏せた添付を取り落とす。
    if (item.kind === "file") {
      const file = item.getAsFile();
      if (file) {
        event.preventDefault();
        void attach(file);
      }
      return;
    }
  }
}

/** 「参照…」。同じファイルを選び直せるよう、読んだら value を戻す。 */
function onFilePicked(event: Event): void {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (file) void attach(file);
}

/**
 * 外から文面を差し込む（Spec 12 — 分岐したときに、選んだ依頼を戻す）。
 *
 * **送信はしない。** 差した文面は書き換えられる状態で置くのが目的で、
 * 分岐の用途はそもそも「別の頼み方を試す」こと。カーソルは末尾へ置く。
 *
 * 下書きの所有権は入力欄に閉じたままにしたいので、`v-model` を親へ生やさず
 * この 1 メソッドだけを公開する。
 */
async function fill(payload: string): Promise<void> {
  text.value = payload;
  closeCompletion();
  await nextTick();
  autoGrow();
  const el = area.value;
  if (!el) return;
  el.focus();
  el.setSelectionRange(payload.length, payload.length);
}

defineExpose({ fill });
</script>

<template>
    <div class="relative shrink-0 border-t border-line px-3 py-2.5" data-tour="chat">
    <!-- 添付チップ（Spec 23）。× で外せる。S4 の注記をすぐ下に置く —
         「1 ターン限り」は設定ではなく仕様なので、添付のたびに見える場所で言う。 -->
    <div v-if="attachment || converting" class="mb-2 px-1">
      <div
        class="inline-flex max-w-full items-center gap-2 rounded-lg bg-surface-1 px-2.5 py-1.5 ring-1 ring-line"
      >
        <!-- 画像アイコン（SVG。絵文字は恒久要素に使わない）。 -->
        <svg
          viewBox="0 0 24 24"
          class="size-4 shrink-0 text-ink-dim"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <circle cx="9" cy="10" r="1.6" />
          <path d="m3 17 5-5 4 4 3-3 6 6" />
        </svg>
        <template v-if="converting">
          <span class="text-[11px] text-ink-dim">{{ $t("chatInput.attachConverting") }}</span>
        </template>
        <template v-else-if="attachment">
          <span class="truncate text-[11px] text-ink" :title="attachment.fileName">
            {{ attachment.fileName }}
          </span>
          <!-- 種別を語で出す（Spec 36）。アイコンは 1 つに畳んであるので、
               何を貼ったかはこのラベルが唯一の手掛かりになる。 -->
          <span class="shrink-0 rounded bg-surface-2 px-1 text-[10px] text-ink-dim">
            {{ $t(`chatInput.attachKind.${attachment.kind}`) }}
          </span>
          <span class="shrink-0 text-[10px] text-ink-dim tabular-nums">
            {{ attachmentDimensions }}{{ attachmentSize }}
          </span>
          <button
            type="button"
            class="shrink-0 rounded px-0.5 text-[12px] leading-none text-ink-dim transition hover:text-warn"
            :title="$t('chatInput.attachRemove')"
            :aria-label="$t('chatInput.attachRemove')"
            @click="attachment = null"
          >
            ×
          </button>
        </template>
      </div>
      <p class="mt-1 text-[10px] text-ink-dim">
        {{ $t("chatInput.attachNote") }}
        <template v-if="attachment?.scaled">
          {{ $t("chatInput.attachScaled", { px: MAX_EDGE_PX }) }}
        </template>
        <!-- S6: コストの桁は貼った時点で言う（請求で知るのでは遅い）。
             **数字は計算せず定型文**で出す — 各社のレートは実測で桁が違い
             （同じ画像で Gemini 1,064 対 OpenAI 130）、表を持つと必ず腐る。 -->
        <template v-if="attachment && attachment.kind !== 'image'">
          {{ $t(`chatInput.attachCost.${attachment.kind}`) }}
        </template>
      </p>
      <!-- 宛先が運べないときの警告（D12）。**送信は止めない** — 止めるのは
           コア側の門で、ここは貼った時点で分かるようにするためだけ。 -->
      <p v-if="carryWarning" class="mt-1 text-[10px] text-warn">
        {{ carryWarning }}
      </p>
    </div>

    <!-- 会話の参照のチップ（Spec 58）。添付のチップと同じ作法で、× で外せる。
         **入力欄の文面には何も挿していない**ので、何を添えたかはこの行が唯一の表示。 -->
    <div v-if="quotes.length" class="mb-2 flex flex-wrap items-center gap-1.5 px-1" data-quote-chips>
      <span
        v-for="chip in quotes"
        :key="chip.id"
        class="inline-flex max-w-full items-center gap-1.5 rounded-lg bg-surface-1 px-2 py-1 ring-1 ring-line"
      >
        <!-- 引用の印（SVG。絵文字は恒久要素に使わない）。 -->
        <svg
          viewBox="0 0 24 24"
          class="size-3.5 shrink-0 text-ink-dim"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M9 14 4 9l5-5" />
          <path d="M4 9h10a6 6 0 0 1 6 6v5" />
        </svg>
        <span class="truncate text-[11px] text-ink">{{ chip.fromName }}</span>
        <span class="shrink-0 text-[10px] text-ink-dim tabular-nums">
          {{ shortTime(chip.tsMs) }} ·
          {{ $t("chatInput.quoteChars", { chars: chip.chars.toLocaleString("en-US") }) }}
        </span>
        <button
          type="button"
          class="shrink-0 rounded px-0.5 text-[12px] leading-none text-ink-dim transition hover:text-warn"
          :title="$t('chatInput.quoteRemove')"
          :aria-label="$t('chatInput.quoteRemove')"
          @click="quotes = quotes.filter((c) => c.id !== chip.id)"
        >
          ×
        </button>
      </span>
    </div>

    <!-- パス補完（Spec 24）。**入力欄の上へ浮かせる**（`absolute` + `bottom-full`）。
         流し込みで置くと、開くたびに入力欄が下へ動いて**会話の表示が縮む** —
         打鍵のたびにレイアウトが跳ねるのは、補完のように頻繁に開閉する部品では
         そのまま使い勝手の悪さになる。浮かせれば会話ログの上に重なるだけで済む。
         左右は `left-3 right-3` で入力欄の `px-3` に揃える。
         横位置はカーソルに追わせない — textarea のカーソル座標を取るには
         ミラー要素が要り、割に合わない。 -->
    <!-- 擦りガラス。**色は生の値ではなくトークンにアルファを掛ける**
         （`bg-surface-1/80`）ので、`color-mix` へ展開されてライトテーマの
         上書きにも自動で追従する（配色は `style.css` の 1 箇所、の規律を保つ）。
         ぼかしを強めに取るのは見た目のためだけではない — 背後の吹き出しや
         アクセント色が透けたまま残ると、10px のフォルダ名の可読性が落ちる。
         **ぼかしが背後を均すことで、透過と可読性が両立する。** -->
    <div
      v-if="popupVisible"
      class="absolute right-3 bottom-full left-3 z-20 mb-1 overflow-hidden rounded-lg bg-surface-1/80 shadow-lg ring-1 ring-line backdrop-blur-xl"
    >
      <!-- 会話の参照（Spec 58）。`@@` のとき、ファイルの代わりにこの会話の発話を出す。 -->
      <template v-if="isQuoteTrigger">
        <p v-if="quotePool.length === 0" class="px-2.5 py-2 text-[11px] text-ink-dim">
          {{ $t("chatInput.quoteNone") }}
        </p>
        <template v-else>
          <ul class="max-h-72 overflow-y-auto py-0.5">
            <li v-for="(item, index) in quoteSuggestions" :key="item.id">
              <!-- 1 行は「送り手 → 宛先・時刻・字数」+ 本文の先頭。**送り手が主** —
                   照合の 1 段目が表示名なので、表示もそれを先頭に置く。 -->
              <button
                type="button"
                class="flex w-full flex-col gap-0.5 px-2.5 py-1 text-left transition disabled:cursor-not-allowed disabled:opacity-50"
                :class="index === selectedIndex ? 'bg-surface-2' : ''"
                :disabled="quotesFull"
                :title="item.head"
                @mouseenter="selectedIndex = index"
                @mousedown.prevent
                @click="confirmQuote(item)"
              >
                <span class="flex items-center gap-1.5">
                  <span class="truncate text-[11px] text-ink">{{ item.fromName }}</span>
                  <span class="shrink-0 text-[10px] text-ink-dim">→ {{ quoteTarget(item) }}</span>
                  <span class="ml-auto shrink-0 text-[10px] text-ink-dim tabular-nums">
                    {{ shortTime(item.tsMs) }} ·
                    {{ $t("chatInput.quoteChars", { chars: item.chars.toLocaleString("en-US") }) }}
                  </span>
                </span>
                <span class="truncate text-[10px] text-ink-dim">{{ item.head }}</span>
              </button>
            </li>
          </ul>
          <!-- 上限。押せない行だけ出して黙ると「`@@` が効かない」と読まれる。 -->
          <p
            v-if="quotesFull"
            class="border-t border-line px-2.5 py-1.5 text-[10px] text-warn"
          >
            {{ $t("chatInput.quoteFull", { max: MAX_QUOTES }) }}
          </p>
        </template>
      </template>
      <!-- 作業フォルダが無い（D3）。候補を出す代わりに理由を出す。 -->
      <p v-else-if="!hasWorkDir" class="px-2.5 py-2 text-[11px] text-ink-dim">
        {{ $t("chatInput.completeNoWorkDir") }}
      </p>
      <p v-else-if="loadingCandidates" class="px-2.5 py-2 text-[11px] text-ink-dim">
        {{ $t("chatInput.completeLoading") }}
      </p>
      <p v-else-if="candidatesFailed" class="px-2.5 py-2 text-[11px] text-warn">
        {{ $t("chatInput.completeFailed") }}
      </p>
      <template v-else>
        <!-- 1 行は「アイコン + ファイル名 + フォルダ（薄く）」。
             **ファイル名が主**なのは、人がそれを打っているから（D5）。
             フルパスを 1 本で出すと、目が先頭のフォルダから読み始めることになる。 -->
        <ul class="max-h-72 overflow-y-auto py-0.5">
          <li v-for="(item, index) in suggestions" :key="item.candidate.id">
            <button
              type="button"
              class="flex w-full items-center gap-2 px-2.5 py-1 text-left transition"
              :class="index === selectedIndex ? 'bg-surface-2' : ''"
              :title="item.candidate.id"
              @mouseenter="selectedIndex = index"
              @mousedown.prevent
              @click="confirmCandidate(item.candidate)"
            >
              <!-- ファイルの印。**絵文字は使わない**（恒久要素の規律。
                   `currentColor` を継承しないとテーマの配色に追従しない）。 -->
              <svg
                viewBox="0 0 24 24"
                class="size-3.5 shrink-0 text-ink-dim"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
                <path d="M14 3v5h5" />
              </svg>
              <span class="truncate text-[11px] text-ink">{{ item.base }}</span>
              <span v-if="item.dir" class="truncate text-[10px] text-ink-dim">
                {{ item.dir }}
              </span>
            </button>
          </li>
        </ul>
        <!-- 打ち切り（D4）。候補に出ないファイルがあるのに黙っていると
             「打っても出ないから無い」と読まれる。 -->
        <p
          v-if="candidatesTruncated"
          class="border-t border-line px-2.5 py-1.5 text-[10px] text-ink-dim"
        >
          {{ $t("chatInput.completeTruncated") }}
        </p>
      </template>
    </div>

    <div
      class="relative rounded-xl bg-surface-1 ring-1 ring-transparent transition focus-within:ring-accent/60"
      :class="{ 'opacity-40': disabled }"
    >
      <textarea
        ref="area"
        v-model="text"
        rows="1"
        :disabled="disabled"
        :placeholder="placeholder"
        data-chat-input

        class="selectable block w-full resize-none bg-transparent py-2.5 pr-12 pl-11 text-[12px] leading-relaxed text-ink outline-none placeholder:text-ink-dim disabled:cursor-not-allowed"
        :style="{ maxHeight: `${MAX_HEIGHT_PX}px`, overflowY: 'auto' }"
        @input="autoGrow(); refreshTrigger()"
        @click="refreshTrigger"
        @keyup="refreshTrigger"
        @blur="closeCompletion"
        @keydown.enter.exact="onEnter"
        @keydown.down="moveSelection(1, $event)"
        @keydown.up="moveSelection(-1, $event)"
        @keydown.esc="onEscape"
        @keydown.tab="onTab"
        @paste="onPaste"
      />

      <!-- 参照…（画像の添付）。送信ボタンと対称の位置。伸びる方向は下なので
           下端に留まる（送信ボタンの bottom-1 と同じ判断）。 -->
      <button
        type="button"
        :disabled="disabled || converting"
        :aria-label="$t('chatInput.attach')"
        :title="$t('chatInput.attach')"
        class="absolute bottom-1 left-1.5 grid size-8 place-items-center rounded-lg text-ink-dim transition hover:bg-surface-2 hover:text-ink disabled:cursor-not-allowed disabled:opacity-40"
        @click="filePicker?.click()"
      >
        <!-- クリップ -->
        <svg
          viewBox="0 0 24 24"
          class="size-4"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path
            d="m21.4 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"
          />
        </svg>
      </button>
      <input
        ref="filePicker"
        type="file"
        accept="image/*,audio/mpeg,audio/mp3,audio/wav,audio/x-wav,video/mp4,application/pdf"
        class="hidden"
        @change="onFilePicked"
      />

      <!-- 送信。中身（本文か添付）があるときだけ現れる。

           下の余白は `bottom-1`（4px）。1 行のときの入力欄は
           `py-2.5`（10px×2）+ 行の高さ 19.5px ≈ 40px で、ボタンは 32px なので
           上下に 4px ずつ残ると中央に来る。`bottom-2`（8px）だとボタンの上端が
           入力欄の上端に接し、**下だけ余って見えた**（実機の指摘）。

           複数行へ伸びたときは下端に寄る — 伸びる方向は下なので、
           **書いている行の隣にボタンが残る**（中央に置くと文章の途中を指す）。 -->
      <button
        v-show="text.trim() || attachment || quotes.length"
        type="button"
        :disabled="!canSend"
        :aria-label="$t('chatInput.send')"
        :title="canSend ? $t('chatInput.sendEnter') : (blockedReason ?? $t('chatInput.cannotSend'))"
        class="absolute right-2 bottom-1 grid size-8 place-items-center rounded-lg bg-accent text-surface-0 transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
        @click="send"
      >
        <!-- ↵ -->
        <svg
          viewBox="0 0 24 24"
          class="size-4"
          fill="none"
          stroke="currentColor"
          stroke-width="2.2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M9 10 4 15l5 5" />
          <path d="M20 4v7a4 4 0 0 1-4 4H4" />
        </svg>
      </button>
    </div>

    <div class="mt-1 flex items-center gap-2 px-1 text-[10px] text-ink-dim">
      <p>{{ $t("chatInput.hint") }}</p>
      <!--
        右端のグループ。**`ml-auto` はグループに付ける**（ボタン自身に付けたまま
        その左に輪を置くと、輪は左寄せ側の末尾に残りボタンだけが右へ押し出される —
        Spec 49 rev2 査読 B-1）。
      -->
      <div class="ml-auto flex items-center gap-2">
        <!--
          表示クリア。**中身は消さない**ので `title` で言い切る（押した人が
          「軽くなった」と読むのを防ぐ — モデルが読む量は 1 バイトも変わらない）。
          アイコンは SVG。絵文字は環境で字形と大きさが変わり `currentColor` を
          継承しないので、恒久要素には使わない（Spec 13 の規律）。
        -->
      <button
        type="button"
        class="rounded p-0.5 text-ink-dim hover:text-accent disabled:opacity-40 disabled:hover:text-ink-dim"
        :disabled="!canClear"
        :title="$t('chatInput.clearView')"
        :aria-label="$t('chatInput.clearView')"
        @click="emit('clearView')"
      >
        <svg
          viewBox="0 0 24 24"
          class="h-3.5 w-3.5"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="m15 5 5 5-8 8H7l-4-4z" />
          <path d="M21 20h-11" />
        </svg>
      </button>
        <!--
          コンテキスト使用率の輪（Spec 49）。**消しゴム（表示クリア）の右**に置く
          （2026-09-04 利用者裁定 — 右端は「いま話しかけている相手の状態」の席）。
          選択中の個体の**直近の 1 呼び出し**の
          入力 ÷ テンプレートの contextLength。色は 3 段のトークン（accent / warn / fail）
          で、SVG は currentColor から引く（生の色を書かない）。弧は 1.0 で止め、
          数字は丸めない — 100% 超は「設定が実際の窓より小さい」の診断（D2）。
        -->
        <span
          v-if="contextUsage"
          :class="['flex items-center gap-1 tabular-nums', contextUsage.tone]"
          :title="
            $t('chatInput.contextUsage', {
              used: (lastPromptTokens ?? 0).toLocaleString(),
              total: (contextLength ?? 0).toLocaleString(),
            }) + (contextUsage.over ? ' ' + $t('chatInput.contextUsageOver') : '')
          "
          data-context-usage
        >
          <svg viewBox="0 0 14 14" class="size-3.5" aria-hidden="true">
            <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" stroke-width="1.6" opacity="0.25" />
            <circle
              cx="7"
              cy="7"
              r="5.5"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              transform="rotate(-90 7 7)"
              :stroke-dasharray="`${contextUsage.arc * RING_CIRCUMFERENCE} ${RING_CIRCUMFERENCE}`"
            />
          </svg>
          <span>{{ contextUsage.percent }}%</span>
        </span>
      </div>
    </div>
  </div>
</template>
