<script setup lang="ts">
/**
 * システム設定ダイアログ。タイトルバーの COG から開く（Spec 13）。
 *
 * 2 ペイン: 左メニューが「設定できるものの目録」そのもの（S2 の実体）で、
 * 右がその設定のページ。**未実装の設定はメニューに並べない** — 目録に載せて
 * 触れないのは、できないことをできると見せる嘘になる（P2 で線削除の確認、
 * P3 で言語が増える）。
 *
 * 保存は「保存」を押したときだけ。触らず閉じたら `world.json` も
 * `localStorage` も書き換わらない（settings_contract の検証項目）。
 */
import { computed, onMounted, ref } from "vue";

import * as ipc from "../lib/ipc";

const emit = defineEmits<{ (e: "close"): void }>();

/** 左メニューの選択。P1 は村の設定 = トークン天井の 1 ページだけ。 */
type Page = "tokenBudget";
const page = ref<Page>("tokenBudget");

const loading = ref(true);
const busy = ref(false);
const error = ref("");
/** 保存が成功した直後の告知。次の操作で消す。 */
const savedNote = ref("");

// ---- トークン天井（村の設定） --------------------------------------------------

/** 保存済みの値。差分検出（触らず閉じたら書かない）の基準。 */
const savedCeiling = ref<number | null>(null);
/** フォームの状態。「天井なし」はラジオで明示的に選ぶ（0 のマジック値を作らない）。 */
const hasCeiling = ref(true);
const ceilingInput = ref<number>(1_000_000);

/** フォームが表す天井。`null` = 天井なし。 */
const formCeiling = computed<number | null>(() =>
  hasCeiling.value ? ceilingInput.value : null,
);

/**
 * 0 と非整数は入力段で弾く（コアの `INVALID_TOKEN_BUDGET` との二重化 —
 * 「保存したのに黙って別の値になる」を画面に作らない）。
 */
const ceilingValid = computed(() => {
  if (!hasCeiling.value) return true;
  return Number.isInteger(ceilingInput.value) && ceilingInput.value >= 1;
});

const dirty = computed(() => formCeiling.value !== savedCeiling.value);

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    const ceiling = await ipc.getTokenBudget();
    savedCeiling.value = ceiling;
    hasCeiling.value = ceiling !== null;
    if (ceiling !== null) ceilingInput.value = ceiling;
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    loading.value = false;
  }
}

onMounted(load);

async function save(): Promise<void> {
  if (!ceilingValid.value || !dirty.value || busy.value) return;
  busy.value = true;
  error.value = "";
  savedNote.value = "";
  try {
    await ipc.setTokenBudget(formCeiling.value);
    savedCeiling.value = formCeiling.value;
    savedNote.value = "保存しました。次の依頼から効きます（再起動は不要です）。";
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[560px] w-[760px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">システム設定</h2>
        <button class="px-1 text-ink-dim hover:text-ink" @click="emit('close')">✕</button>
      </header>

      <div class="flex min-h-0 flex-1">
        <!-- 左メニュー = 設定できるものの目録（S2）。 -->
        <nav class="w-44 shrink-0 overflow-y-auto border-r border-line bg-surface-0 py-2 text-[11px]">
          <p class="px-3 pb-1 pt-1 font-semibold text-ink-dim">村の設定</p>
          <button
            class="menu-item"
            :class="{ active: page === 'tokenBudget' }"
            @click="page = 'tokenBudget'"
          >
            トークン天井
          </button>
        </nav>

        <!-- 右ページ -->
        <div class="min-h-0 flex-1 overflow-y-auto p-4 text-[11px]">
          <p v-if="loading" class="py-8 text-center text-ink-dim">読み込み中…</p>

          <template v-else-if="page === 'tokenBudget'">
            <h3 class="mb-1 text-xs font-semibold text-ink">トークン天井（実効トークン）</h3>
            <!-- ヘルプ文言は settings_contract で凍結（rev3 D1 — 素の値は出さない）。 -->
            <p class="mb-3 text-ink-dim">
              未キャッシュ×1 + キャッシュ済み×0.1 + 出力×4 を合算した値。内部では
              milli 単位で管理し、表示は切り上げた整数です。依頼 1 つあたりの
              トークン消費がこの値に達すると、その依頼は自動で打ち切られます。
            </p>

            <p v-if="error" class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-fail">
              {{ error }}
            </p>

            <div class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <label class="flex items-center gap-2">
                <input v-model="hasCeiling" type="radio" :value="true" />
                <span>天井あり</span>
                <input
                  v-model.number="ceilingInput"
                  type="number"
                  min="1"
                  step="1"
                  :disabled="!hasCeiling"
                  class="w-32 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent disabled:opacity-40"
                />
                <span class="text-ink-dim">実効トークン / 依頼</span>
              </label>
              <p v-if="hasCeiling && !ceilingValid" class="pl-6 text-fail">
                1 以上の整数を入力してください（0 で天井を外すことはできません —
                外すには「天井なし」を選びます）。
              </p>
              <p class="pl-6 text-ink-dim">
                目安: 6 体前後の村 = 1,000,000 / 12 体規模・出力多めのコード生成 = 2,000,000〜3,000,000
              </p>

              <label class="flex items-center gap-2">
                <input v-model="hasCeiling" type="radio" :value="false" />
                <span>天井なし</span>
              </label>
              <!-- 天井なしはその場で赤く示す（機構 3 — 起動 WARN を待たせない）。 -->
              <p v-if="!hasCeiling" class="pl-6 text-fail">
                天井のない村では、失敗ループが起きたとき依頼 1 つでトークンを
                使い続けます。上限を設けることを推奨します。
              </p>

              <div class="flex items-center justify-end gap-2 pt-1">
                <span v-if="savedNote" class="text-run">{{ savedNote }}</span>
                <button
                  class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!ceilingValid || !dirty || busy"
                  @click="save"
                >
                  {{ busy ? "保存中…" : "保存" }}
                </button>
              </div>
            </div>
          </template>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.menu-item {
  display: block;
  width: 100%;
  padding: 5px 12px;
  text-align: left;
  color: var(--color-ink-dim, #8b93a7);
  background: transparent;
  border: none;
  cursor: pointer;
}
.menu-item:hover {
  color: var(--color-ink, #e6e9f2);
  background: color-mix(in oklab, currentColor 8%, transparent);
}
.menu-item.active {
  color: var(--color-ink, #e6e9f2);
  background: color-mix(in oklab, currentColor 12%, transparent);
  font-weight: 600;
}
</style>
