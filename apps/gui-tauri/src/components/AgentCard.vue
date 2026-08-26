<script setup lang="ts">
/**
 * 左ペインのエージェントカード 1 枚。
 *
 * メタデータは KV リストで並べる。ラベル幅を固定しているのは、
 * カードが縦に積まれたとき値の位置が揃わないと一覧として読めないため。
 */
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import { avatarHue, avatarInitial } from "../lib/avatar";
import type { ToolRun } from "../lib/chatRows";
import { formatError } from "../lib/errorText";
import { compactNumber, exactNumber } from "../lib/format";
import { roleBadge } from "../lib/roleLabel";
import { toolLabel } from "../lib/toolLabel";
import { useOrchestrator } from "../composables/useOrchestrator";
import { STATUS_LABEL_KEYS, type AgentSnapshot } from "../types";

const { t } = useI18n();

const props = defineProps<{
  agent: AgentSnapshot;
  selected: boolean;
  /** 設定済みアイコンの object URL。無ければ頭文字の円を出す。 */
  icon?: string | null;
  /** 直近に実行したツール。まだ 1 度も呼んでいなければ未指定。 */
  lastTool?: ToolRun | null;
}>();

const emit = defineEmits<{
  (e: "select"): void;
  (e: "configure"): void;
  /** 失敗理由の枠を閉じる（表示だけ。コアの `last_error` は残る）。 */
  (e: "dismiss-error"): void;
  /** スタンバイボタン: この個体の実際の起動・停止。 */
  (e: "toggle", running: boolean): void;
  /** 対象トグル: 一括起動（▶）に含めるか。稼働状態は変えない。 */
  (e: "batch-start", included: boolean): void;
}>();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

/** 失敗理由の枠を出すか。判定は composable の 1 実装を借りる。 */
const showsError = computed(() => orchestrator.showsError(props.agent));

/**
 * 枠の `title`。**詳細と操作説明を両方出す** — 詳細だけだと閉じられることが
 * 伝わらず、操作説明だけだと `detail` を読む唯一の経路が消える
 * （本文は `formatError` の要約で、原文はここにしか無い）。
 */
const errorTitle = computed(() => {
  const detail = props.agent.lastError?.detail;
  const hint = t("agentCard.dismissErrorTitle");
  return detail ? `${detail}\n\n${hint}` : hint;
});

/**
 * 役職バッジの文言。引けなければ `null` で、**バッジごと描かない**
 * （`role_contract` 凍結 5 — `[不明]` とは書かない）。
 *
 * **このバッジは由来であって、現在の中身を保証しない**（機構 8）。作成後に
 * `Construct.md` も道具も手で変えられるので、「調査役」のまま中身が別の個体は
 * 正当な操作で生まれる。`title` にもそう書く — 画面の言葉と仕様の言葉を分けない。
 */
const role = computed(() => roleBadge(props.agent.roleId, state.roles));

/** 状態に対応する表示色。停止・稼働・失敗が一目で分かることを優先する。 */
const statusColor = computed(() => {
  switch (props.agent.status) {
    case "running":
      return "bg-run";
    case "starting":
    case "stopping":
      return "bg-warn";
    case "failed":
      return "bg-fail";
    default:
      return "bg-line";
  }
});

/** 稼働中とみなす状態。トグルの見た目はこれで決める。 */
const isOn = computed(
  () => props.agent.status === "running" || props.agent.status === "starting",
);

/**
 * 遷移中（起動処理中・停止処理中）。この間は電源ボタンを封じる —
 * 起動の完了を待つ間にもう一度押すと、起きた直後の個体を止める二度押しに
 * なる（2026-08-27 利用者指摘）。`starting` は MCP 接続のタイムアウト
 * （30 秒）で必ず抜けるので、押せない時間は有界。
 */
const transitioning = computed(
  () => props.agent.status === "starting" || props.agent.status === "stopping",
);

/** 電源ボタンの hover 文言。遷移中は「押せない理由」を出す。 */
const powerTitle = computed(() => {
  switch (props.agent.status) {
    case "starting":
      return t("agentCard.starting", { name: props.agent.name });
    case "stopping":
      return t("agentCard.stopping", { name: props.agent.name });
    default:
      return isOn.value
        ? t("agentCard.stop", { name: props.agent.name })
        : t("agentCard.start", { name: props.agent.name });
  }
});

/** 秒数を `1h 02m 03s` 形式にする。 */
const uptime = computed(() => {
  const total = props.agent.uptimeSecs;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}h ${pad(m)}m ${pad(s)}s` : `${m}m ${pad(s)}s`;
});

/** 大きな数を読みやすく丸める。狭い枠なので桁区切りではなく短縮表記にする。 */
const tokens = computed(() => compactNumber(props.agent.totalTokens));

/**
 * 直近ツールの表示指示（2026-08-08）。
 *
 * **カードは宛先の表示名を引けない** — `AgentCard` は自分 1 体しか知らないので、
 * `ask_agent_3` の相手が誰かを解決できない。**id のまま出す**のが正直で、
 * 会話ペイン（村ぜんぶを知っている側）だけが名前へ解ける。
 */
const lastToolLabel = computed(() =>
  props.lastTool ? toolLabel(props.lastTool.tool, (id) => id) : null,
);

/** カードに出すツール名。外部は識別子のまま、それ以外は辞書から引く。 */
const lastToolName = computed(() => {
  const label = lastToolLabel.value;
  if (!label) return "";
  return label.kind === "external" ? label.id : t(label.key, { name: label.target ?? "" });
});

/** 直近ツールの hover 表示。狭い欄に入らない情報（時刻・成否・識別子）をここへ逃がす。 */
const lastToolTitle = computed(() => {
  const run = props.lastTool;
  if (!run) return undefined;
  const at = new Date(run.tsMs).toLocaleTimeString("ja-JP", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
  // 識別子を併記するのは、撮った画面から `fuseforks.log` の `name=` を引けるようにするため。
  const named = `${lastToolName.value}（${run.tool}）`;
  return run.ok
    ? t("agentCard.lastToolRanAt", { time: at, tool: named })
    : t("agentCard.lastToolRanAtFailed", { time: at, tool: named });
});

/**
 * プロンプトキャッシュから読まれた割合。まだ 1 度も喋っていなければ `null`。
 *
 * 合計だけでは、**キャッシュが一度も効いていない状態と完全に効いている状態が
 * 同じ数字に見える**。実機で 5 体全員が無キャッシュのまま数日走っており、
 * 気づいたのは請求ダッシュボードのグラフからだった。設定を変えた次のターンで
 * 効果が分かるように、ここへ出す。
 */
const cacheRate = computed(() => {
  // **分母は入力だけ。** 出力はキャッシュできないので、合計を分母にすると
  // 天井が 100% にならず「あと何が取り残されているか」が読めない。
  const input = props.agent.promptTokens;
  if (input <= 0) return null;
  return Math.round((props.agent.cachedTokens / input) * 100);
});

/** 出力が占める割合。キャッシュ率の天井を理解するための補助情報。 */
const outputShare = computed(() => {
  const total = props.agent.totalTokens;
  if (total <= 0) return null;
  return Math.round(((total - props.agent.promptTokens) / total) * 100);
});

/** 割合に応じた色。0% は警告色にする — 設定の取りこぼしである可能性が高い。 */
const cacheTone = computed(() => {
  const rate = cacheRate.value;
  if (rate === null) return "text-ink-dim";
  if (rate === 0) return "text-warn";
  return "text-ink-dim";
});
</script>

<template>
  <article
    class="cursor-pointer rounded-md border px-3 py-2.5 transition-colors"
    :class="
      selected
        ? 'border-accent bg-surface-2'
        : 'border-line bg-surface-1 hover:bg-surface-2'
    "
    @click="emit('select')"
  >
    <header class="flex items-center gap-2">
      <!-- アバター + 状態。状態ドットはアバターの右下に重ね、面積を取らずに両方見せる。 -->
      <div class="relative shrink-0">
        <img
          v-if="icon"
          :src="icon"
          class="size-8 rounded-full object-cover ring-1 ring-line"
          :alt="agent.name"
        />
        <div
          v-else
          class="flex size-8 items-center justify-center rounded-full text-[12px] font-semibold text-surface-0"
          :style="{ backgroundColor: avatarHue(agent.name) }"
        >
          {{ avatarInitial(agent.name) }}
        </div>
        <span
          class="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-surface-1"
          :class="statusColor"
          :title="$t(STATUS_LABEL_KEYS[agent.status])"
        />
      </div>
      <h3 class="min-w-0 truncate font-medium">{{ agent.name }}</h3>
      <!-- 役職バッジ（Spec 14）。名前の直後に置く — 「誰が」の一部だから。 -->
      <span
        v-if="role"
        class="shrink-0 rounded border px-1 py-px text-[10px] leading-none"
        :class="role.color ? '' : 'border-line text-ink-dim'"
        :style="role.color ? { borderColor: role.color, color: role.color } : undefined"
        :title="$t('roles.badgeTitle', { name: role.name })"
      >
        {{ role.name }}
      </span>
      <span class="min-w-0 flex-1"></span>

      <!-- 設定。カード本体のクリック（選択）と混ざらないよう伝播を止める。 -->
      <button
        class="shrink-0 rounded px-1 py-0.5 text-ink-dim hover:text-accent"
        :title="$t('agentCard.openSettings')"
        @click.stop="emit('configure')"
      >
        <svg
          width="15"
          height="15"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
        </svg>
      </button>

      <!--
        一括起動の対象トグル。**稼働状態ではない。**

        以前はこのスイッチが起動・停止そのものだったが、それだと
        「今は止まっているが、次の一括起動では起こしたい」を表現できず、
        起動のたびに 1 体ずつ押す必要があった。ここは「▶ で起こす顔ぶれ」の
        選択（永続）で、実際の起動は右隣のスタンバイボタンが担う。
      -->
      <button
        role="switch"
        :aria-checked="agent.batchStart"
        :title="
          agent.batchStart
            ? $t('agentCard.batchIncluded')
            : $t('agentCard.batchExcluded')
        "
        class="relative h-5 w-9 shrink-0 rounded-full transition-colors"
        :class="agent.batchStart ? 'bg-accent' : 'bg-line'"
        @click.stop="emit('batch-start', !agent.batchStart)"
      >
        <span
          class="absolute top-0.5 size-4 rounded-full bg-surface-0 transition-all"
          :class="agent.batchStart ? 'left-4.5' : 'left-0.5'"
        />
      </button>

      <!-- スタンバイ（電源）。この個体の実際の起動・停止。
           色は状態と一致させる（稼働中は緑、停止中は淡色）。 -->
      <button
        :title="powerTitle"
        :disabled="transitioning"
        class="shrink-0 rounded-full p-1 transition-colors"
        :class="
          transitioning
            ? 'cursor-default text-warn opacity-60'
            : isOn
              ? 'text-run hover:bg-surface-2'
              : 'text-ink-dim hover:text-ink hover:bg-surface-2'
        "
        @click.stop="emit('toggle', !isOn)"
      >
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          aria-hidden="true"
        >
          <path d="M18.36 6.64a9 9 0 1 1-12.73 0" />
          <line x1="12" y1="2" x2="12" y2="12" />
        </svg>
      </button>
    </header>

    <dl class="mt-2 grid grid-cols-[72px_minmax(0,1fr)] gap-x-2 gap-y-1 text-[11px]">
      <dt class="text-ink-dim">{{ $t("agentCard.model") }}</dt>
      <dd class="truncate" :title="agent.model">{{ agent.model }}</dd>

      <dt class="text-ink-dim">{{ $t("agentCard.uptime") }}</dt>
      <dd class="tabular-nums">{{ uptime }}</dd>

      <dt class="text-ink-dim">{{ $t("agentCard.tokens") }}</dt>
      <dd class="tabular-nums">
        {{ tokens }}
        <span
          v-if="cacheRate !== null"
          :class="cacheTone"
          :title="$t('agentCard.cacheDetail', {
            total: exactNumber(agent.totalTokens),
            input: exactNumber(agent.promptTokens),
            output: exactNumber(agent.totalTokens - agent.promptTokens),
            outputShare,
            cached: exactNumber(agent.cachedTokens),
          })"
          >{{ $t("agentCard.cacheRate", { rate: cacheRate }) }}</span
        >
      </dd>

      <dt class="text-ink-dim">RAG</dt>
      <dd class="truncate">
        {{ agent.ragSources.length ? agent.ragSources.join(", ") : "—" }}
      </dd>

      <dt class="text-ink-dim">{{ $t("agentCard.connections") }}</dt>
      <dd class="tabular-nums">{{ $t("agentCard.connectionsCount", { count: agent.connectedAgents.length }) }}</dd>

      <!--
        直近に実行したツール。**「今この個体が何をしているか」**を出す欄で、
        履歴を遡る場所ではない（そちらは会話ペインの時系列に混ざる）。
        ツールの結果はプロンプトの中で消えるため、この欄が無いと
        「黙って副作用だけ起きた」状態を一覧から読み取れない。
      -->
      <template v-if="lastTool">
        <dt class="text-ink-dim">{{ $t("agentCard.lastTool") }}</dt>
        <dd class="truncate" :title="lastToolTitle">
          <span
            class="mr-1 inline-block size-1.5 rounded-full align-middle"
            :class="lastTool.ok ? 'bg-run' : 'bg-fail'"
          />
          <span :class="lastToolLabel?.kind === 'external' ? 'font-mono' : ''">{{ lastToolName }}</span>
        </dd>
      </template>
    </dl>

    <!--
      失敗理由はカード内に残す。トースト任せにすると閉じた瞬間に原因が消える。
      **クリックで閉じられる**（2026-08-11）— 復帰したあとも前回の失敗が
      居座り、今の状態と食い違って見えるため。閉じるのは画面の 1 枚だけで、
      コアの `last_error` も `fuseforks.log` の `turn failed:` 行も残る。
      **次の失敗では自動的に開き直る**（`agentFailed` が閉じた印を外す）。
    -->
    <button
      v-if="showsError"
      type="button"
      class="mt-2 w-full cursor-pointer rounded border border-fail/40 bg-fail/10 px-2 py-1 text-left text-[11px] text-fail hover:bg-fail/20"
      :title="errorTitle"
      :aria-label="t('agentCard.dismissError')"
      @click.stop="emit('dismiss-error')"
    >
      {{ formatError(agent.lastError!) }}
    </button>
  </article>
</template>
