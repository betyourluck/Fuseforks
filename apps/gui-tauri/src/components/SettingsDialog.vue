<script setup lang="ts">
/**
 * システム設定ダイアログ。タイトルバーの COG から開く（Spec 13）。
 *
 * 2 ペイン: 左メニューが「設定できるものの目録」そのもの（S2 の実体）で、
 * 右がその設定のページ。**未実装の設定はメニューに並べない** — 目録に載せて
 * 触れないのは、できないことをできると見せる嘘になる。
 *
 * **分類は主題で切る**（全般 / コスト管理 / ユーザーインターフェース）。初版は
 * 保存先（`world.json` か `localStorage` か）で切っていたが、それは実装の都合が
 * そのまま目録に出た形で読みにくかった（2026-08-03 の実機指摘）。保存先は
 * 分類ではなく各ページの注記で示す — 村に保存される設定は配布すると付いて回る、
 * という利用者に実害のある事実だけが伝わればよい。
 *
 * 保存の挙動もページごとに正直に書く: 村の設定（天井・言語）は「保存」を押した
 * ときだけ IPC で書き、端末側の設定（メッセージ表示/非表示）は即保存。
 * 触らず閉じたら `world.json` も `localStorage` も書き換わらない。
 */
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { avatarHue, avatarInitial } from "../lib/avatar";
import { formatError } from "../lib/errorText";
import { fileToWebpIcon } from "../lib/iconImage";
import { setLocale } from "../i18n";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { useUiSettings, type Theme } from "../composables/useUiSettings";
import type { AgentId, Language } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state } = orchestrator;

/**
 * 左メニューの選択。既定は**目録の先頭**（settings_contract）。
 *
 * 指す先は Spec 19 で「全般 → 言語」から「全般 → ユーザー」へ動いた。
 * **規則（先頭）は不変で、指す先だけが動く。**
 */
type Page = "user" | "language" | "tokenBudget" | "mcpHost" | "theme" | "messages";
const page = ref<Page>("user");

/**
 * 村（`world.json`）に保存されるページ。読み込みに IPC が要る側でもあるので、
 * 「読み込み中…」の覆いと村の注記はこの 1 つの定義から引く。
 */
const VILLAGE_PAGES: Page[] = ["user", "language", "tokenBudget", "mcpHost"];
const isVillagePage = computed(() => VILLAGE_PAGES.includes(page.value));

/** 端末側の設定（localStorage）。チェックの変更は watch が即座に保存する。 */
const { settings } = useUiSettings();

/** 配色の選択肢。`Theme` と 1 対 1（増やしたら `style.css` にも足す）。 */
const THEME_OPTIONS: Theme[] = ["dark", "light"];

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

// ---- 利用者の呼び名（村の設定。Spec 19） --------------------------------------

/** 保存済みの呼び名。`null` = 未設定。差分検出の基準。 */
const savedUserName = ref<string | null>(null);
/** フォームの状態。空文字は「未設定へ戻す」を表す。 */
const userNameInput = ref("");

/** フォームが表す呼び名。空白だけの入力は未設定と同じ扱いにする。 */
const formUserName = computed<string | null>(() => {
  const trimmed = userNameInput.value.trim();
  return trimmed === "" ? null : trimmed;
});

const userNameDirty = computed(() => formUserName.value !== savedUserName.value);

// ---- 利用者のアイコン（Spec 19） -----------------------------------------------

const userIconInput = ref<HTMLInputElement | null>(null);
const iconBusy = ref(false);
const iconError = ref("");

/** アバターの頭文字と色は未設定時の表示名から引く（3 画面共通の規則）。 */
const avatarName = computed(() => formUserName.value ?? t("chat.you"));

async function onUserIconPicked(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  // 同じファイルを選び直しても change が飛ぶよう、値は毎回捨てる。
  input.value = "";
  if (!file) return;

  iconBusy.value = true;
  iconError.value = "";
  try {
    const bytes = await fileToWebpIcon(file);
    await orchestrator.setUserIcon(bytes);
  } catch (e) {
    // 変換の失敗は画像側の問題なので、村の設定の error とは別枠に出す。
    iconError.value = e instanceof Error ? e.message : t("agentSettings.iconConvertFailed");
  } finally {
    iconBusy.value = false;
  }
}

async function removeUserIcon(): Promise<void> {
  const ok = await askConfirm({
    title: t("settings.user.iconRemoveTitle"),
    message: t("settings.user.iconRemoveMessage"),
  });
  if (!ok) return;
  await orchestrator.clearUserIcon();
}

// ---- MCP サーバー（扉。Spec 25） ----------------------------------------------

/**
 * 扉の状態。**真実は共有状態**（`state.mcpHost`）で、ここはその読み口。
 * ステータスバーも同じ値を読むので、片方だけが古くなる形を作らない。
 *
 * **保存済みの値と入力欄を分けない** — このページの操作（ON/OFF・ポート）は
 * 押した時点で反映する形にしてあり、「保存していない状態」を持たない。
 * トークン天井と規律が違うのは、扉は**開いたか開かないかが結果**で、
 * 押すまで分からないものを溜めても意味が無いため。
 */
const mcpHost = computed(() => state.mcpHost);
/** ポートだけは入力中の値を持つ（打っている途中で bind し直さない）。 */
const portInput = ref(39641);
const tokenCopied = ref(false);

/** 窓口（`world.json`）。`null` = 未設定。 */
const receptionInput = ref<AgentId | "">("");

const portValid = computed(
  () => Number.isInteger(portInput.value) && portInput.value >= 1024 && portInput.value <= 65535,
);

/** 扉の設定を適用する（ON にした時点で合鍵ができる）。 */
async function applyMcpHost(enabled: boolean): Promise<void> {
  if (busy.value || !portValid.value) return;
  busy.value = true;
  error.value = "";
  savedNote.value = "";
  tokenCopied.value = false;
  try {
    await orchestrator.setMcpHost(enabled, portInput.value);
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

async function regenerateToken(): Promise<void> {
  if (busy.value) return;
  const ok = await askConfirm({
    title: t("settings.mcpHost.regenerate"),
    message: t("settings.mcpHost.regenerateConfirm"),
  });
  if (!ok) return;
  busy.value = true;
  error.value = "";
  tokenCopied.value = false;
  try {
    await orchestrator.regenerateMcpHostToken();
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

async function copyToken(): Promise<void> {
  const token = mcpHost.value?.token;
  if (!token) return;
  await navigator.clipboard.writeText(token);
  tokenCopied.value = true;
}

/** 窓口を保存する。**選んだ瞬間に反映する**（保存ボタンを置かない）。 */
async function saveReception(): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.setReception(receptionInput.value === "" ? null : receptionInput.value);
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

/** クライアントへ貼る設定の例。**合鍵をそのまま埋める**（貼るための値なので）。 */
const clientConfigSample = computed(() => {
  const token = mcpHost.value?.token ?? "<token>";
  const port = mcpHost.value?.port ?? portInput.value;
  return JSON.stringify(
    {
      mcpServers: {
        concordia: {
          type: "http",
          url: `http://127.0.0.1:${port}/mcp`,
          headers: { Authorization: `Bearer ${token}` },
        },
      },
    },
    null,
    2,
  );
});

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

    // 呼び名は起動時に投影へ載っているが、ここでも引き直す — このダイアログは
    // 別の窓で変えられた後に開かれることがある。
    const userName = await ipc.getUserName();
    savedUserName.value = userName;
    userNameInput.value = userName ?? "";

    // 扉（Spec 25）。状態は起動時に共有状態へ載っているので、ここでは
    // ポート入力の初期値を合わせるだけ。
    portInput.value = state.mcpHost?.port ?? portInput.value;
    receptionInput.value = (await ipc.getReception()) ?? "";
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
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
    error.value = formatError(payload);
  } finally {
    busy.value = false;
  }
}

/**
 * 呼び名を保存する（Spec 19）。
 *
 * **書式の検査はコアに任せる**（空 / `】` / 制御文字 / 32 字超）。フロントで
 * 先回りすると同じ規律が 2 箇所に生え、片方だけ直したときに画面とコアが食い違う。
 * 天井の 0 を入力段で弾いているのとは事情が違う — あちらは「保存したのに黙って
 * 別の値になる」正規化を打ち消すためで、こちらは拒否がそのまま返ってくる。
 */
async function saveUserName(): Promise<void> {
  if (!userNameDirty.value || busy.value) return;
  busy.value = true;
  error.value = "";
  savedNote.value = "";
  try {
    // **`orchestrator` 経由で呼ぶ**（生の `ipc` ではなく）— 会話ペインの投影
    // `state.userName` を同時に更新するため。`mutate` は例外を飲んで `false` を
    // 返すので、**成否は戻り値で見る**（`saveLanguage` の生 IPC + try/catch とは
    // 経路が違う）。拒否の理由は `mutate` が通知で出す。
    if (await orchestrator.setUserName(formUserName.value)) {
      savedUserName.value = formUserName.value;
      // 保存できた場合だけ正規化後の値を欄へ映す（前後の空白が消える）。
      userNameInput.value = formUserName.value ?? "";
      savedNote.value = t("settings.user.saved");
    }
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
    error.value = formatError(payload);
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
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
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
          <p class="px-3 pb-1 pt-1 font-semibold text-ink-dim">{{ $t("settings.groupGeneral") }}</p>
          <button
            class="menu-item"
            :class="{ active: page === 'user' }"
            @click="selectPage('user')"
          >
            {{ $t("settings.menuUser") }}
          </button>
          <button
            class="menu-item"
            :class="{ active: page === 'language' }"
            @click="selectPage('language')"
          >
            {{ $t("settings.menuLanguage") }}
          </button>

          <p class="px-3 pb-1 pt-3 font-semibold text-ink-dim">{{ $t("settings.groupCost") }}</p>
          <button
            class="menu-item"
            :class="{ active: page === 'tokenBudget' }"
            @click="selectPage('tokenBudget')"
          >
            {{ $t("settings.menuTokenLimit") }}
          </button>

          <p class="px-3 pb-1 pt-3 font-semibold text-ink-dim">
            {{ $t("settings.groupIntegration") }}
          </p>
          <button
            class="menu-item"
            :class="{ active: page === 'mcpHost' }"
            @click="selectPage('mcpHost')"
          >
            {{ $t("settings.menuMcpHost") }}
          </button>

          <p class="px-3 pb-1 pt-3 font-semibold text-ink-dim">{{ $t("settings.groupUi") }}</p>
          <button
            class="menu-item"
            :class="{ active: page === 'theme' }"
            @click="selectPage('theme')"
          >
            {{ $t("settings.menuTheme") }}
          </button>
          <button
            class="menu-item"
            :class="{ active: page === 'messages' }"
            @click="selectPage('messages')"
          >
            {{ $t("settings.menuMessages") }}
          </button>
        </nav>

        <!-- 右ページ -->
        <div class="min-h-0 flex-1 overflow-y-auto p-4 text-[11px]">
          <!-- 読み込み待ちは IPC を持つページ（村に保存される側）だけ。
               localStorage のページは即描く。 -->
          <p v-if="loading && isVillagePage" class="py-8 text-center text-ink-dim">
            {{ $t("settings.loading") }}
          </p>

          <!-- 全般 > ユーザー（Spec 19） -->
          <template v-else-if="page === 'user'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.user.heading") }}
            </h3>
            <p class="mb-3 text-ink-dim">{{ $t("settings.user.help") }}</p>

            <p v-if="error" class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-fail">
              {{ error }}
            </p>

            <!-- アイコン。丸抜きのプレビューが会話ペインと同じ見た目になる。 -->
            <div class="mb-3 flex items-center gap-3 rounded border border-line bg-surface-0 p-3">
              <img
                v-if="state.userIcon"
                :src="state.userIcon"
                class="size-14 shrink-0 rounded-full object-cover ring-1 ring-line"
                :alt="$t('settings.user.iconAlt')"
              />
              <div
                v-else
                class="flex size-14 shrink-0 items-center justify-center rounded-full text-lg font-semibold text-surface-0"
                :style="{ backgroundColor: avatarHue(avatarName) }"
              >
                {{ avatarInitial(avatarName) }}
              </div>

              <div class="min-w-0">
                <div class="flex gap-2">
                  <button
                    class="rounded border border-line px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
                    :disabled="iconBusy"
                    @click="userIconInput?.click()"
                  >
                    {{
                      iconBusy
                        ? $t("agentSettings.iconConverting")
                        : state.userIcon
                          ? $t("agentSettings.iconChange")
                          : $t("agentSettings.iconChoose")
                    }}
                  </button>
                  <button
                    v-if="state.userIcon"
                    class="rounded border border-fail/60 px-2 py-1 text-fail hover:bg-fail/10"
                    @click="removeUserIcon"
                  >
                    {{ $t("agentSettings.delete") }}
                  </button>
                </div>
                <p class="mt-1 text-ink-dim">{{ $t("agentSettings.iconHint") }}</p>
                <p v-if="iconError" class="mt-0.5 text-fail">{{ iconError }}</p>
              </div>

              <input
                ref="userIconInput"
                type="file"
                accept="image/*"
                class="hidden"
                @change="onUserIconPicked"
              />
            </div>

            <!-- 呼び名。**保存は明示的**（アイコンは即保存）— 入力中の 1 文字ごとに
                 封筒が変わると、途中の名前でサーヴァントに呼ばれる周ができる。 -->
            <div class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <label class="block">
                <span class="mb-1 block text-ink-dim">{{ $t("settings.user.nameLabel") }}</span>
                <input
                  v-model="userNameInput"
                  type="text"
                  :placeholder="$t('chat.you')"
                  class="w-64 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                  @keyup.enter="saveUserName"
                />
              </label>
              <p class="text-ink-dim">{{ $t("settings.user.nameHint") }}</p>

              <div class="flex items-center justify-end gap-2 pt-1">
                <span v-if="savedNote" class="text-run">{{ savedNote }}</span>
                <button
                  class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!userNameDirty || busy"
                  @click="saveUserName"
                >
                  {{ busy ? $t("settings.language.saving") : $t("settings.language.save") }}
                </button>
              </div>
            </div>
            <p class="mt-2 text-ink-dim">{{ $t("settings.villageScope") }}</p>
          </template>

          <!-- 全般 > 言語 -->
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
            <p class="mt-2 text-ink-dim">{{ $t("settings.villageScope") }}</p>
          </template>

          <!-- コスト管理 -->
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
              <!-- 制限なしはその場で赤く示す（機構 3 — 起動 WARN を待たせない）。 -->
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
            <p class="mt-2 text-ink-dim">{{ $t("settings.villageScope") }}</p>
          </template>

          <!--
            外部連携 > MCP サーバー（Spec 25）。**押した時点で反映する** —
            扉は「開いたか開かないか」が結果なので、保存を溜めると
            ポートが埋まっていたことに気づくのが遅れる。
          -->
          <template v-else-if="page === 'mcpHost'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.mcpHost.heading") }}
            </h3>
            <p class="mb-3 text-ink-dim">{{ $t("settings.mcpHost.lead") }}</p>

            <p
              v-if="error"
              class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-fail"
            >
              {{ error }}
            </p>
            <!-- 設定ファイルが読めないときは、保存できない理由をそのまま出す。 -->
            <p
              v-if="mcpHost?.blocked"
              class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-fail"
            >
              {{ mcpHost.blocked }}
            </p>

            <div class="space-y-3 rounded border border-line bg-surface-0 p-3">
              <label class="flex items-center gap-2">
                <input
                  type="checkbox"
                  :checked="mcpHost?.enabled ?? false"
                  :disabled="busy || !portValid || !!mcpHost?.blocked"
                  @change="applyMcpHost(($event.target as HTMLInputElement).checked)"
                />
                <span>{{ $t("settings.mcpHost.enable") }}</span>
              </label>
              <p class="pl-6 text-ink-dim">{{ $t("settings.mcpHost.enableHint") }}</p>

              <!-- **ON と「待ち受け中」を別に出す。** ポートが埋まっていると
                   設定は ON のまま開いていない状態になる。 -->
              <p v-if="mcpHost?.listening" class="pl-6 text-run">
                {{ $t("settings.mcpHost.listening", { port: mcpHost.port }) }}
              </p>
              <p v-else-if="mcpHost?.enabled" class="pl-6 text-fail">
                {{ $t("settings.mcpHost.notListening") }}
              </p>
              <p v-if="mcpHost?.lastError" class="selectable pl-6 text-fail">
                {{ mcpHost.lastError }}
              </p>

              <label class="flex items-center gap-2">
                <span class="w-20 text-ink-dim">{{ $t("settings.mcpHost.port") }}</span>
                <input
                  v-model.number="portInput"
                  type="number"
                  min="1024"
                  max="65535"
                  step="1"
                  class="w-28 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                  @change="mcpHost?.enabled && applyMcpHost(true)"
                />
              </label>
              <p class="pl-[5.5rem] text-ink-dim">{{ $t("settings.mcpHost.portHint") }}</p>

              <div class="space-y-1">
                <p class="text-ink-dim">{{ $t("settings.mcpHost.token") }}</p>
                <div v-if="mcpHost?.token" class="flex items-center gap-2">
                  <code
                    class="selectable min-w-0 flex-1 truncate rounded border border-line bg-surface-1 px-2 py-1"
                  >
                    {{ mcpHost.token }}
                  </code>
                  <button class="rounded border border-line px-2 py-1" @click="copyToken">
                    {{ tokenCopied ? $t("settings.mcpHost.copied") : $t("settings.mcpHost.copy") }}
                  </button>
                  <button
                    class="rounded border border-line px-2 py-1"
                    :disabled="busy"
                    @click="regenerateToken"
                  >
                    {{ $t("settings.mcpHost.regenerate") }}
                  </button>
                </div>
                <p v-else class="text-ink-dim">{{ $t("settings.mcpHost.tokenNotYet") }}</p>
                <p class="text-ink-dim">{{ $t("settings.mcpHost.tokenHint") }}</p>
              </div>

              <div class="space-y-1">
                <label class="flex items-center gap-2">
                  <span class="w-20 text-ink-dim">{{ $t("settings.mcpHost.reception") }}</span>
                  <select
                    v-model="receptionInput"
                    class="rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                    :disabled="busy"
                    @change="saveReception"
                  >
                    <option value="">{{ $t("settings.mcpHost.receptionNone") }}</option>
                    <option v-for="agent in state.agents" :key="agent.id" :value="agent.id">
                      {{ agent.name }}
                    </option>
                  </select>
                </label>
                <p class="text-ink-dim">{{ $t("settings.mcpHost.receptionHint") }}</p>
              </div>

              <div v-if="mcpHost?.token" class="space-y-1">
                <p class="text-ink-dim">{{ $t("settings.mcpHost.clientConfig") }}</p>
                <pre
                  class="selectable overflow-x-auto rounded border border-line bg-surface-1 p-2 text-[10px]"
                  >{{ clientConfigSample }}</pre
                >
              </div>

              <p class="text-ink-dim">{{ $t("settings.mcpHost.concurrencyNote") }}</p>
            </div>
            <p class="mt-2 text-ink-dim">{{ $t("settings.mcpHost.scopeNote") }}</p>
          </template>

          <!--
            テーマ。**選んだ瞬間に反映する**（保存ボタンを持たない）— 見た目の
            設定は結果を見て決めるものなので、押すまで変わらないと選べない。
          -->
          <template v-else-if="page === 'theme'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.theme.heading") }}
            </h3>
            <p class="mb-3 text-ink-dim">{{ $t("settings.theme.intro") }}</p>

            <div class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <label
                v-for="option in THEME_OPTIONS"
                :key="option"
                class="flex items-start gap-2"
              >
                <input v-model="settings.theme" type="radio" :value="option" class="mt-0.5" />
                <span>
                  <span class="text-ink">{{ $t(`settings.theme.${option}`) }}</span>
                  <span class="block text-ink-dim">
                    {{ $t(`settings.theme.${option}Note`) }}
                  </span>
                </span>
              </label>
            </div>
            <p class="mt-2 text-ink-dim">{{ $t("settings.theme.deviceNote") }}</p>
          </template>

          <!--
            ユーザーインターフェース: 非表示にできるメッセージの一覧。
            **節を足せば項目が増える形**にしてあるが、機構としての切り替えは
            いまも線の削除 1 つだけ（settings_contract）。1 つ足すたびに
            「便利さと安全のどちらかを機械が決められない」の検討が要る。
          -->
          <template v-else-if="page === 'messages'">
            <h3 class="mb-1 text-xs font-semibold text-ink">
              {{ $t("settings.messages.heading") }}
            </h3>
            <p class="mb-3 text-ink-dim">{{ $t("settings.messages.intro") }}</p>

            <section class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <h4 class="font-semibold text-ink">
                {{ $t("settings.messages.edgeDelete.heading") }}
              </h4>
              <p class="text-ink-dim">{{ $t("settings.messages.edgeDelete.intro") }}</p>
              <label class="flex items-center gap-2">
                <input v-model="settings.confirmEdgeDelete" type="checkbox" />
                <span>{{ $t("settings.messages.edgeDelete.checkbox") }}</span>
              </label>
              <p v-if="!settings.confirmEdgeDelete" class="pl-6 text-warn">
                {{ $t("settings.messages.edgeDelete.offWarning") }}
              </p>
            </section>
            <p class="mt-2 text-ink-dim">{{ $t("settings.messages.instantNote") }}</p>
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
  color: var(--color-ink-dim);
  background: transparent;
  border: none;
  cursor: pointer;
}
.menu-item:hover {
  color: var(--color-ink);
  background: color-mix(in oklab, currentColor 8%, transparent);
}
.menu-item.active {
  color: var(--color-ink);
  background: color-mix(in oklab, currentColor 12%, transparent);
  font-weight: 600;
}
</style>
