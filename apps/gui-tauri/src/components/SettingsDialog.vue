<script setup lang="ts">
/**
 * システム設定ダイアログ。タイトルバーの COG から開く（Spec 13）。
 *
 * 2 ペイン: 左メニューが「設定できるものの目録」そのもの（S2 の実体）で、
 * 右がその設定のページ。**未実装の設定はメニューに並べない** — 目録に載せて
 * 触れないのは、できないことをできると見せる嘘になる。
 *
 * 保存の挙動はページごとに正直に書く: 村の設定（天井・言語）は「保存」を押した
 * ときだけ IPC で書き、この画面の設定（線削除の確認）は即保存。
 * 触らず閉じたら `world.json` も `localStorage` も書き換わらない。
 */
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { setLocale } from "../i18n";
import { useUiSettings } from "../composables/useUiSettings";
import type { Language } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();

/** 左メニューの選択。村の設定 = 天井・言語 / この画面の設定 = 線削除の確認。 */
type Page = "tokenBudget" | "language" | "edgeConfirm";
const page = ref<Page>("tokenBudget");

/** この画面の設定（localStorage）。チェックの変更は watch が即座に保存する。 */
const { settings } = useUiSettings();

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

const ceilingDirty = computed(() => formCeiling.value !== savedCeiling.value);

// ---- 言語（村の設定） ----------------------------------------------------------

/** 保存済みの言語。差分検出の基準。 */
const savedLanguage = ref<Language>("ja");
const languageInput = ref<Language>("ja");

const languageDirty = computed(() => languageInput.value !== savedLanguage.value);

// ---- 読み書き ------------------------------------------------------------------

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    const ceiling = await ipc.getTokenBudget();
    savedCeiling.value = ceiling;
    hasCeiling.value = ceiling !== null;
    if (ceiling !== null) ceilingInput.value = ceiling;

    const language = await ipc.getLanguage();
    savedLanguage.value = language;
    languageInput.value = language;
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    loading.value = false;
  }
}

onMounted(load);

async function saveCeiling(): Promise<void> {
  if (!ceilingValid.value || !ceilingDirty.value || busy.value) return;
  busy.value = true;
  error.value = "";
  savedNote.value = "";
  try {
    await ipc.setTokenBudget(formCeiling.value);
    savedCeiling.value = formCeiling.value;
    savedNote.value = t("settings.tokenBudget.saved");
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    busy.value = false;
  }
}

async function saveLanguage(): Promise<void> {
  if (!languageDirty.value || busy.value) return;
  busy.value = true;
  error.value = "";
  savedNote.value = "";
  try {
    await ipc.setLanguage(languageInput.value);
    savedLanguage.value = languageInput.value;
    // 保存できてから表示を切り替える。切り替えてから保存に失敗すると、
    // 画面と world.json の言語が食い違ったまま残る。
    setLocale(languageInput.value);
    savedNote.value = t("settings.language.saved");
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    busy.value = false;
  }
}

function selectPage(next: Page): void {
  page.value = next;
  savedNote.value = "";
  error.value = "";
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
        <h2 class="flex-1 font-semibold">{{ $t("settings.title") }}</h2>
        <button
          class="px-1 text-ink-dim hover:text-ink"
          :aria-label="$t('settings.close')"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <div class="flex min-h-0 flex-1">
        <!-- 左メニュー = 設定できるものの目録（S2）。 -->
        <nav class="w-44 shrink-0 overflow-y-auto border-r border-line bg-surface-0 py-2 text-[11px]">
          <p class="px-3 pb-1 pt-1 font-semibold text-ink-dim">{{ $t("settings.groupVillage") }}</p>
          <button
            class="menu-item"
            :class="{ active: page === 'tokenBudget' }"
            @click="selectPage('tokenBudget')"
          >
            {{ $t("settings.menuTokenBudget") }}
          </button>
          <button
            class="menu-item"
            :class="{ active: page === 'language' }"
            @click="selectPage('language')"
          >
            {{ $t("settings.menuLanguage") }}
          </button>
          <p class="px-3 pb-1 pt-3 font-semibold text-ink-dim">{{ $t("settings.groupScreen") }}</p>
          <button
            class="menu-item"
            :class="{ active: page === 'edgeConfirm' }"
            @click="selectPage('edgeConfirm')"
          >
            {{ $t("settings.menuEdgeConfirm") }}
          </button>
        </nav>

        <!-- 右ページ -->
        <div class="min-h-0 flex-1 overflow-y-auto p-4 text-[11px]">
          <!-- 読み込み待ちは IPC を持つページ（村の設定）だけ。localStorage のページは即描く。 -->
          <p
            v-if="loading && (page === 'tokenBudget' || page === 'language')"
            class="py-8 text-center text-ink-dim"
          >
            {{ $t("settings.loading") }}
          </p>

          <template v-else-if="page === 'tokenBudget'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.tokenBudget.heading") }}
            </h3>
            <!-- ヘルプ文言は settings_contract で凍結（rev3 D1 — 素の値は出さない）。 -->
            <p class="mb-3 text-ink-dim">{{ $t("settings.tokenBudget.help") }}</p>

            <p v-if="error" class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-fail">
              {{ error }}
            </p>

            <div class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <label class="flex items-center gap-2">
                <input v-model="hasCeiling" type="radio" :value="true" />
                <span>{{ $t("settings.tokenBudget.hasCeiling") }}</span>
                <input
                  v-model.number="ceilingInput"
                  type="number"
                  min="1"
                  step="1"
                  :disabled="!hasCeiling"
                  class="w-32 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent disabled:opacity-40"
                />
                <span class="text-ink-dim">{{ $t("settings.tokenBudget.unit") }}</span>
              </label>
              <p v-if="hasCeiling && !ceilingValid" class="pl-6 text-fail">
                {{ $t("settings.tokenBudget.invalid") }}
              </p>
              <p class="pl-6 text-ink-dim">{{ $t("settings.tokenBudget.guideline") }}</p>

              <label class="flex items-center gap-2">
                <input v-model="hasCeiling" type="radio" :value="false" />
                <span>{{ $t("settings.tokenBudget.noCeiling") }}</span>
              </label>
              <!-- 天井なしはその場で赤く示す（機構 3 — 起動 WARN を待たせない）。 -->
              <p v-if="!hasCeiling" class="pl-6 text-fail">
                {{ $t("settings.tokenBudget.noCeilingWarning") }}
              </p>

              <div class="flex items-center justify-end gap-2 pt-1">
                <span v-if="savedNote" class="text-run">{{ savedNote }}</span>
                <button
                  class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!ceilingValid || !ceilingDirty || busy"
                  @click="saveCeiling"
                >
                  {{ busy ? $t("settings.tokenBudget.saving") : $t("settings.tokenBudget.save") }}
                </button>
              </div>
            </div>
          </template>

          <template v-else-if="page === 'language'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.language.heading") }}
            </h3>
            <p class="mb-3 text-ink-dim">{{ $t("settings.language.help") }}</p>

            <p v-if="error" class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-fail">
              {{ error }}
            </p>

            <div class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <!-- 言語名は常に自国語で書く（日本語 / English）。訳さない —
                   いま読めない言語の一覧から自分の言語を探す人が読むため。 -->
              <label class="flex items-center gap-2">
                <input v-model="languageInput" type="radio" value="ja" />
                <span>日本語</span>
              </label>
              <label class="flex items-center gap-2">
                <input v-model="languageInput" type="radio" value="en" />
                <span>English</span>
              </label>

              <p class="text-ink-dim">{{ $t("settings.language.promptNote") }}</p>

              <div class="flex items-center justify-end gap-2 pt-1">
                <span v-if="savedNote" class="text-run">{{ savedNote }}</span>
                <button
                  class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!languageDirty || busy"
                  @click="saveLanguage"
                >
                  {{ busy ? $t("settings.language.saving") : $t("settings.language.save") }}
                </button>
              </div>
            </div>
          </template>

          <template v-else-if="page === 'edgeConfirm'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.edgeConfirm.heading") }}
            </h3>
            <p class="mb-3 text-ink-dim">{{ $t("settings.edgeConfirm.intro") }}</p>

            <div class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <label class="flex items-center gap-2">
                <input v-model="settings.confirmEdgeDelete" type="checkbox" />
                <span>{{ $t("settings.edgeConfirm.checkbox") }}</span>
              </label>
              <p v-if="!settings.confirmEdgeDelete" class="pl-6 text-warn">
                {{ $t("settings.edgeConfirm.offWarning") }}
              </p>
              <p class="pl-6 text-ink-dim">{{ $t("settings.edgeConfirm.instantNote") }}</p>
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
