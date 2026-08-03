<script setup lang="ts">
/**
 * 会話（セッション）の一覧ダイアログ。会話ペインの「会話一覧」から開く（Spec 12）。
 *
 * 開く・分岐する・書き出す・消す、の 4 つだけ。新規チャットは会話ペインの
 * ボタンが担うので、ここには置かない（同じ操作の入口を 2 つ作らない）。
 *
 * **切り替えは飛行中のターンがあると拒否される**。自動で打ち切ってから
 * 切り替えることはしない（畳むかどうかは人の判断）。拒否の理由と直し方は
 * コアのエラー文言（`SESSION_SWITCH_BLOCKED`）がそのまま通知に出る。
 */
import { computed, onMounted, ref } from "vue";

import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { ForkPoint } from "../types";

const emit = defineEmits<{
  (e: "close"): void;
  /**
   * 分岐したので、選んだ依頼を入力欄へ差し戻してほしい。
   *
   * 依頼そのものは複製先に残していない（残すと返事の付かない依頼が宙に浮く）。
   * 書き換えて送るところまでが分岐の操作なので、文面は入力欄に置く。
   */
  (e: "prefill", payload: { text: string; to: string | null }): void;
}>();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const loading = ref(true);
const busy = ref(false);

/** 分岐点を選んでいる会話。`null` なら一覧を出している。 */
const forking = ref<string | null>(null);
const forkPoints = ref<ForkPoint[]>([]);

const sessions = computed(() => state.sessions);

onMounted(async () => {
  await orchestrator.refreshSessions();
  loading.value = false;
});

/** 一覧に出す時刻。日付をまたぐ判断が要るので日付ごと出す。 */
function stamp(ms: number): string {
  return new Date(ms).toLocaleString("ja-JP", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function titleOf(id: string, title: string): string {
  // 表題は最初のユーザー発話から採るので、まだ誰も話していない会話は空になる。
  return title || `（まだ発話のない会話 ${id.slice(-8)}）`;
}

async function open(sessionId: string): Promise<void> {
  if (busy.value || sessionId === state.currentSessionId) return;
  busy.value = true;
  const ok = await orchestrator.resumeSession(sessionId);
  busy.value = false;
  if (ok) emit("close");
}

async function beginFork(sessionId: string): Promise<void> {
  busy.value = true;
  forkPoints.value = await orchestrator.forkPoints(sessionId);
  forking.value = sessionId;
  busy.value = false;
}

async function fork(point: ForkPoint): Promise<void> {
  if (!forking.value || busy.value) return;
  busy.value = true;
  const ok = await orchestrator.forkSession(forking.value, point.atSeq);
  busy.value = false;
  if (!ok) return;
  emit("prefill", { text: point.text, to: point.to ?? null });
  emit("close");
}

async function remove(sessionId: string, title: string): Promise<void> {
  if (busy.value) return;
  const label = titleOf(sessionId, title);
  const ok = await askConfirm({
    title: `「${label}」を削除しますか？`,
    message: "この会話の発話と履歴は元に戻せません。残したいなら先に「書き出し」を。",
    confirmLabel: "削除する",
    danger: true,
  });
  if (!ok) return;
  busy.value = true;
  await orchestrator.deleteSession(sessionId);
  busy.value = false;
}

async function exportOne(sessionId: string): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  await orchestrator.exportSession(sessionId);
  busy.value = false;
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[560px] w-[720px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">
          {{ forking ? "分岐する地点を選ぶ" : "会話" }}
        </h2>
        <button
          v-if="forking"
          class="rounded border border-line px-1.5 py-0.5 text-[10px] text-ink-dim hover:border-accent hover:text-accent"
          @click="forking = null"
        >
          ← 一覧へ戻る
        </button>
        <button class="px-1 text-ink-dim hover:text-ink" @click="emit('close')">✕</button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        <template v-if="forking">
          選んだ依頼を出す<strong class="text-ink">直前まで</strong>を複製し、その依頼の文面を
          入力欄へ戻します。書き換えて送れば、別の頼み方を試せます。
          元の会話はそのまま残ります。
        </template>
        <template v-else>
          会話は再起動をまたいで残ります。開き直すと、サーヴァントも前の話を踏まえて答えます。
          <strong class="text-warn">切り替えは、飛行中のターンが無いときだけできます。</strong>
        </template>
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">読み込み中…</p>

        <!-- 分岐点の選択 -->
        <template v-else-if="forking">
          <p
            v-if="!forkPoints.length"
            class="rounded border border-line bg-surface-0 p-3 text-[11px] text-ink-dim"
          >
            分岐できる地点がありません。分岐は「その依頼を出す直前へ戻る」操作なので、
            会話のいちばん先頭の依頼は選べません（その手前は空の会話で、
            それは「新規チャット」と同じになります）。
          </p>
          <ul v-else class="space-y-1.5">
            <li v-for="point in forkPoints" :key="point.atSeq">
              <button
                class="flex w-full items-center gap-2 rounded border border-line bg-surface-0 p-2 text-left text-[11px] transition hover:border-accent disabled:opacity-40"
                :disabled="busy"
                @click="fork(point)"
              >
                <span class="shrink-0 tabular-nums text-ink-dim">{{ stamp(point.tsMs) }}</span>
                <span class="min-w-0 flex-1 truncate">{{ point.preview }}</span>
                <span class="shrink-0 text-accent">この依頼の直前まで →</span>
              </button>
            </li>
          </ul>
        </template>

        <!-- 会話の一覧 -->
        <template v-else>
          <p
            v-if="!sessions.length"
            class="rounded border border-line bg-surface-0 p-3 text-[11px] text-ink-dim"
          >
            保存された会話はまだありません。
          </p>
          <ul v-else class="space-y-2">
            <li
              v-for="session in sessions"
              :key="session.id"
              class="rounded border bg-surface-0 p-2 text-[11px]"
              :class="
                session.id === state.currentSessionId
                  ? 'border-accent'
                  : 'border-line'
              "
            >
              <div class="flex items-center gap-2">
                <span class="min-w-0 flex-1 truncate font-semibold text-ink">
                  {{ titleOf(session.id, session.meta.title) }}
                </span>
                <span
                  v-if="session.id === state.currentSessionId"
                  class="shrink-0 rounded border border-accent px-1 text-[10px] text-accent"
                >
                  表示中
                </span>
              </div>
              <div class="mt-1 flex items-center gap-2 text-[10px] text-ink-dim">
                <span class="tabular-nums">{{ stamp(session.meta.updatedAt) }}</span>
                <span class="tabular-nums">{{ session.meta.recordCount }} 件</span>
                <!-- 分岐で生まれた会話は、どこから枝分かれしたかを出す。
                     出さないと同じ表題が並んだときに区別が付かない。 -->
                <span v-if="session.meta.parentId" class="text-accent">
                  分岐（seq {{ session.meta.forkedAtSeq }} まで）
                </span>

                <span class="ml-auto flex shrink-0 gap-1">
                  <button
                    class="rounded border border-line px-1.5 py-0.5 transition hover:border-accent hover:text-accent disabled:opacity-40"
                    :disabled="busy || session.id === state.currentSessionId"
                    title="この会話を開きます（飛行中のターンがあると拒否されます）"
                    @click="open(session.id)"
                  >
                    開く
                  </button>
                  <button
                    class="rounded border border-line px-1.5 py-0.5 transition hover:border-accent hover:text-accent disabled:opacity-40"
                    :disabled="busy"
                    title="ある地点までを複製して、そこから別の頼み方を試します"
                    @click="beginFork(session.id)"
                  >
                    分岐
                  </button>
                  <button
                    class="rounded border border-line px-1.5 py-0.5 transition hover:border-accent hover:text-accent disabled:opacity-40"
                    :disabled="busy"
                    title="JSONL で書き出します（保存先はバイナリなので、読める出口を用意しています）"
                    @click="exportOne(session.id)"
                  >
                    書き出し
                  </button>
                  <button
                    class="rounded border border-line px-1.5 py-0.5 text-fail transition hover:border-fail disabled:opacity-40"
                    :disabled="busy"
                    title="この会話を削除します（元に戻せません）"
                    @click="remove(session.id, session.meta.title)"
                  >
                    削除
                  </button>
                </span>
              </div>
            </li>
          </ul>
        </template>
      </div>
    </div>
  </div>
</template>
