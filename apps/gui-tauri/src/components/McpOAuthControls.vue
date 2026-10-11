<script setup lang="ts">
/**
 * OAuth のエントリの状態と操作（Spec 67 D7）。共通 MCP のダイアログと、サーヴァントの編集ダイアログの
 * MCP の状態の行の中に置く。OAuth でないエントリ（`auth: "none"`）には何も描かない。
 *
 * - 「認可する」: ブラウザで認可 URL を開き、戻りを待つ（最長 10 分）。待っている間は残り時間と
 *   「キャンセル」を出す。**ブラウザを開くのはこのボタンを押したときだけ**（D6）
 * - 「認可を取り消す」: 確認してから、手元の記録を消し、接続先へ失効を頼む
 *
 * 終わったら `changed` を出す — 親が接続の状態を読み直す（繋ぎ直しはコアが済ませている）。
 */
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import type { McpServerStatus } from "../types";

const props = defineProps<{ status: McpServerStatus }>();
const emit = defineEmits<{ (e: "changed"): void }>();

const { t } = useI18n();

/** 待ちの上限（ホストの `AUTHORIZE_TIMEOUT` と同じ 10 分。残り時間の表示だけに使う）。 */
const TIMEOUT_MS = 10 * 60 * 1000;

const waiting = ref(false);
const busy = ref(false);
const message = ref("");
const failed = ref(false);
const startedAt = ref(0);
const now = ref(Date.now());
let timer: ReturnType<typeof setInterval> | null = null;

const isOAuth = computed(() => props.status.auth !== "none" && !!props.status.url);
const minutesLeft = computed(() =>
  Math.max(1, Math.ceil((TIMEOUT_MS - (now.value - startedAt.value)) / 60000)),
);

function stopTimer(): void {
  if (timer) clearInterval(timer);
  timer = null;
}

onBeforeUnmount(() => {
  stopTimer();
  // ダイアログを閉じても認可は続く（受け口はホストが持つ）。閉じたことでは取り消さない。
});

async function authorize(): Promise<void> {
  const url = props.status.url;
  if (!url) return;
  message.value = "";
  failed.value = false;
  waiting.value = true;
  startedAt.value = Date.now();
  now.value = startedAt.value;
  timer = setInterval(() => (now.value = Date.now()), 15000);
  try {
    const outcome = await ipc.mcpOAuthAuthorize(props.status.name, url);
    switch (outcome.kind) {
      case "authorized":
        message.value = t("mcp.oauth.outcomeAuthorized");
        break;
      case "cancelled":
        message.value = t("mcp.oauth.outcomeCancelled");
        break;
      case "timedOut":
        message.value = t("mcp.oauth.outcomeTimedOut");
        failed.value = true;
        break;
      case "denied":
        message.value = t("mcp.oauth.outcomeDenied");
        failed.value = true;
        break;
      case "failed":
        message.value = t("mcp.oauth.outcomeFailed", { message: outcome.message });
        failed.value = true;
        break;
    }
  } catch (error) {
    message.value = t("mcp.oauth.outcomeFailed", {
      message: String((error as { message?: string }).message ?? error),
    });
    failed.value = true;
  } finally {
    waiting.value = false;
    stopTimer();
    emit("changed");
  }
}

async function cancel(): Promise<void> {
  if (props.status.url) await ipc.mcpOAuthCancel(props.status.url);
}

async function revoke(): Promise<void> {
  const url = props.status.url;
  if (!url) return;
  const ok = await askConfirm({
    title: t("mcp.oauth.revokeConfirmTitle"),
    message: t("mcp.oauth.revokeConfirmMessage", { server: props.status.name }),
    confirmLabel: t("mcp.oauth.revoke"),
    danger: true,
  });
  if (!ok) return;
  busy.value = true;
  message.value = "";
  failed.value = false;
  try {
    const outcome = await ipc.mcpOAuthRevoke(props.status.name, url);
    message.value = outcome.remoteRevoked
      ? t("mcp.oauth.revokedRemote")
      : t("mcp.oauth.revokedLocal");
  } catch (error) {
    message.value = String((error as { message?: string }).message ?? error);
    failed.value = true;
  } finally {
    busy.value = false;
    emit("changed");
  }
}
</script>

<template>
  <div v-if="isOAuth" class="mt-1 space-y-1 pl-4 text-[11px]">
    <div class="flex flex-wrap items-center gap-2">
      <span
        class="rounded px-1.5 py-0.5"
        :class="status.auth === 'authorized' ? 'bg-run/15 text-run' : 'bg-warn/15 text-warn'"
      >
        {{ status.auth === "authorized" ? $t("mcp.oauth.authorized") : $t("mcp.oauth.authRequired") }}
      </span>
      <template v-if="waiting">
        <span class="text-ink-dim">{{ $t("mcp.oauth.waiting", { minutes: minutesLeft }) }}</span>
        <button
          type="button"
          class="rounded border border-line px-2 py-0.5 text-ink hover:bg-surface-2"
          @click="cancel"
        >
          {{ $t("mcp.oauth.cancel") }}
        </button>
      </template>
      <template v-else>
        <button
          type="button"
          class="rounded border border-line px-2 py-0.5 text-ink hover:bg-surface-2 disabled:opacity-50"
          :disabled="busy"
          :title="$t('mcp.oauth.authorizeTitle')"
          @click="authorize"
        >
          {{ $t("mcp.oauth.authorize") }}
        </button>
        <button
          v-if="status.auth === 'authorized'"
          type="button"
          class="rounded border border-line px-2 py-0.5 text-ink-dim hover:bg-surface-2 hover:text-fail disabled:opacity-50"
          :disabled="busy"
          :title="$t('mcp.oauth.revokeTitle')"
          @click="revoke"
        >
          {{ $t("mcp.oauth.revoke") }}
        </button>
      </template>
    </div>
    <p v-if="message" :class="failed ? 'text-fail' : 'text-ink-dim'">{{ message }}</p>
  </div>
</template>
