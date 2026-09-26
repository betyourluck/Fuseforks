<script setup lang="ts">
/**
 * フッターのステータスバー。日付と時刻、その右に版番号を出す。
 *
 * # なぜ置くか
 *
 * これは管理ツールなので、**画面を撮ったときに「いつの状態か」が写っている**
 * 必要がある（利用者要望 2026-08-03）。撮った後で思い出せない情報は、
 * スクリーンショットを証拠として使えなくする。
 *
 * 版番号も同じ理由で置く（利用者要望 2026-08-05）。**時刻だけでは「いつ」しか
 * 分からず、「どのビルドの」が抜ける** — 不具合の報告を受けたとき、その画面が
 * どのコードのものかが分からないと再現に取り掛かれない。この 2 つは
 * 「常に見る必要があるか」を満たす（下の「左側が空いていること」の規律）。
 *
 * 形式を `fuseforks.log` の行頭（`diag.rs` の `%Y-%m-%d %H:%M:%S%.3f`）と
 * 揃えてあるので、**撮った画面の時刻からログの該当行を目で引ける**。
 * 整形の詳細と、言語に追従させない理由は `lib/clock.ts` に書いた。
 *
 * # 更新の刻み
 *
 * 1 秒。毎回 `new Date()` を読み直すので、`setInterval` の遅れは累積しない
 * （スリープ復帰後もその場で正しい時刻に戻る）。秒まで出すのは、ログ側が
 * ミリ秒精度で、分までだと突き合わせの候補が 1 分ぶん残るため。
 *
 * # 左側
 *
 * MCP サーバー（Spec 25）が待ち受けている間だけ、その旨を出す。
 * 起点は利用者 —「**誰もいないのにつけっぱなしにしないように**」。
 *
 * ここは「常に見えている細い帯」で、常駐させるだけの価値がある情報しか
 * 置かない器（増やすときは**常に見る必要があるか**を毎回問う）。扉が満たすのは、
 * **開いていることが他のどの画面にも出ない**から — 設定を開くまで気づけず、
 * 気づけない状態のまま同じ端末の任意のプロセスが村へ依頼を投げられる。
 *
 * **開いているときだけ出す。** OFF のときに「OFF です」と出すと、印が常に
 * 画面にあることになり、**印が付いていること自体の意味が消える**。
 *
 * **`enabled` ではなく `listening` で出す。** 設定が ON でもポートが埋まって
 * いれば開いていない。ここで見せたいのは「実際に受け付けている」ことで、
 * 「そう設定してある」ことではない（食い違いの診断は設定ページが担う）。
 *
 * # 計画の確認を飛ばすスイッチ（Spec 53・2026-09-15）
 *
 * **この帯の操作は 2 つになった**（統計の入口と、これ）。置くのは統計の左。
 * スイッチは人の確認をまとめて外す状態なので、**オンの間は注意色で発光させ、
 * 帯を見ればオンだと分かる**ようにする（MCP の扉と同じ「つけっぱなしに気づける」
 * 理由 — 状態はコアのメモリだけにあり、再起動で必ず OFF に戻る）。
 *
 * **2026-09-27 に「保存しない」を覆した**（利用者裁定）— コアはメモリだけに持つままだが、
 * 画面が端末の `localStorage` に覚え、起動時に設定し直す（`lib/switchMemory.ts`）。
 *
 * # コマンドの承認モード（Spec 61・2026-09-27）
 *
 * `run` の許可リストに無い呼び出しの扱いを 3 つから選ぶ（承認が必要 / 自動承認して許可 /
 * 承認せずに許可）。**禁止（deny）はどのモードでも実行しない。** 既定以外の間は
 * 計画の確認のスイッチと同じく発光させる — 自動承認は注意色、承認せずに許可は
 * 失敗色（記録も残らないので、より強く知らせる）。
 *
 * # 統計への入口（Spec 39・2026-08-16 に TitleBar から移した）
 *
 * 時計の左にアイコンだけで置く。タイトルバーへ置いて
 * いたときは、ダイアログの入口 6 つの列に**面ごと差し替える 1 つ**が混ざり、
 * 同じ見た目で振る舞いが違った（利用者判断）。ここなら列の性質が割れない。
 *
 * **常駐の規律（常に見る必要があるか）は情報に掛かるもので、ここは操作**。
 * 絆の地図の Auto Fit を Controls へ置いたときと同じ線引き — 押した後は
 * 押さないもの・面の切り替えは、面積を奪う「操作の列」を作らない。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { formatClock } from "../lib/clock";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { RunApproval } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

/** 計画の確認を飛ばすスイッチ（Spec 53）を反転する。失敗は `mutate` がトーストへ。 */
function toggleBypass(): void {
  void orchestrator.setPlanReviewBypass(!state.planReviewBypass);
}

/** コマンドの承認モード（Spec 61）。失敗は `mutate` がトーストへ。 */
function changeRunApproval(value: string): void {
  void orchestrator.setRunApproval(value as RunApproval);
}

const RUN_APPROVAL_OPTIONS: { value: RunApproval; key: string }[] = [
  { value: "required", key: "statusBar.runApprovalRequired" },
  { value: "auto_approve", key: "statusBar.runApprovalAutoApprove" },
  { value: "no_approval", key: "statusBar.runApprovalNoApproval" },
];

const props = defineProps<{ statsActive?: boolean }>();
const emit = defineEmits<{ (e: "toggle-stats"): void }>();

/** 扉が実際に開いているか（Spec 25）。 */
const listening = computed(() => state.mcpHost?.listening === true);

/** 現在時刻。1 秒ごとにティッカーが差し替える。 */
const now = ref(new Date());
const clock = computed(() => formatClock(now.value));

/**
 * 版番号。ビルド時に `vite.config.ts` が git の直近タグから埋め込む定数で、
 * 実行時には変わらない（`ref` にしない）。タグが無ければ `0.0.0`。
 *
 * **時刻と同じく、言語には追従させない。** 版番号は語ではなく識別子で、
 * 読み手の国で表記が変わると報告と突き合わせられなくなる。
 */
const version = __APP_VERSION__;

/**
 * `datetime` 属性は機械可読な形（ISO 8601）で渡す。画面に出す文字列は
 * ローカル時刻の固定形式なので、そのままでは読み手（支援技術・将来の抽出）に
 * タイムゾーンが伝わらない。無効な `Date` では属性ごと落とす。
 */
const machineTime = computed(() => {
  const t = now.value.getTime();
  return Number.isFinite(t) ? now.value.toISOString() : undefined;
});

let timer: ReturnType<typeof setInterval> | undefined;

onMounted(() => {
  timer = setInterval(() => {
    now.value = new Date();
  }, 1000);
});

// 破棄で必ず止める。止め忘れたティッカーは、画面から消えた後も
// 毎秒 ref を書き換え続ける（開発中の HMR で積み上がる）。
onBeforeUnmount(() => {
  if (timer !== undefined) clearInterval(timer);
});
</script>

<template>
  <footer
    class="flex h-[22px] shrink-0 select-none items-center border-t border-line bg-surface-1 px-3 text-[11px] text-ink-dim"
  >
    <!--
      MCP サーバーが待ち受けている間だけ左端に出す（Spec 25）。
      **開いているときだけ**なので、印があること自体が信号になる。
      点は色だけに頼らない補助（帯が細いので、文言と併せて読ませる）。
    -->
    <span
      v-if="listening"
      class="mr-auto flex items-center gap-1.5 text-accent"
      :title="$t('statusBar.mcpHostTitle', { port: state.mcpHost?.port ?? 0 })"
    >
      <span class="size-1.5 rounded-full bg-accent" aria-hidden="true" />
      {{ $t("statusBar.mcpHost", { port: state.mcpHost?.port ?? 0 }) }}
    </span>
    <!-- 扉が閉じている間は左が空くので、右寄せを保つ詰め物を置く。 -->
    <span v-else class="mr-auto" />
    <!--
      計画の確認を飛ばすスイッチ（Spec 53）。オンの間は注意色で発光し、字も出す —
      人の確認が外れていることは、アイコンの色だけでなく言葉で読めるようにする。
    -->
    <button
      type="button"
      class="bypass-btn"
      :class="{ 'is-on': state.planReviewBypass }"
      :title="$t(state.planReviewBypass ? 'statusBar.bypassOnTitle' : 'statusBar.bypassOffTitle')"
      :aria-label="$t('statusBar.bypassAria')"
      :aria-pressed="state.planReviewBypass ? 'true' : 'false'"
      data-plan-review-bypass
      @click="toggleBypass"
    >
      <svg
        width="13"
        height="13"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M5 4l10 8-10 8V4z" />
        <path d="M19 5v14" />
      </svg>
      <span v-if="state.planReviewBypass">{{ $t("statusBar.bypassOnLabel") }}</span>
    </button>
    <!--
      コマンドの承認モード（Spec 61）。3 つから選ぶので select。既定以外の間は発光する。
    -->
    <label
      class="run-approval"
      :class="{
        'is-auto': state.runApproval === 'auto_approve',
        'is-none': state.runApproval === 'no_approval',
      }"
      :title="$t('statusBar.runApprovalTitle')"
      data-run-approval
    >
      <svg
        width="13"
        height="13"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="m4 17 6-6-6-6" />
        <path d="M12 19h8" />
      </svg>
      <select
        :value="state.runApproval"
        :aria-label="$t('statusBar.runApprovalAria')"
        @change="changeRunApproval(($event.target as HTMLSelectElement).value)"
      >
        <option v-for="o in RUN_APPROVAL_OPTIONS" :key="o.value" :value="o.value">
          {{ $t(o.key) }}
        </option>
      </select>
    </label>
    <!--
      統計（Spec 39）。字を持たずアイコンだけ。
      開いている間は緑（`--color-run` = 稼働の色）に発光させる — 押せる場所が
      1 つしか無い帯では、点いているかどうかが状態そのものを指す。
    -->
    <button
      type="button"
      class="stats-btn"
      :class="{ 'is-on': props.statsActive }"
      :title="$t('statusBar.statsTitle')"
      :aria-label="$t('statusBar.statsTitle')"
      :aria-pressed="props.statsActive ? 'true' : 'false'"
      data-stats-toggle
      data-tour="stats"
      @click="emit('toggle-stats')"
    >
      <svg
        width="13"
        height="13"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M4 20V10M10 20V4M16 20v-7M22 20H2" />
      </svg>
    </button>
    <!--
      tabular-nums は必須。等幅でないと桁の太さが毎秒変わり、
      1 秒ごとに時計の幅が揺れて隣が動く。
    -->
    <time
      class="selectable tabular-nums"
      :datetime="machineTime"
      :title="$t('statusBar.clockTitle')"
      :aria-label="$t('statusBar.clockAria')"
    >
      {{ clock }}
    </time>
    <!--
      版番号は時計の右。実行時に変わらないので tabular-nums は要らないが、
      桁の揃った字形のほうが時計と並べたときに帯として落ち着く。
      ラベルは訳さない（版番号は語ではなく識別子で、報告と突き合わせる対象）。
    -->
    <span
      class="selectable ml-3 tabular-nums"
      :title="$t('statusBar.versionTitle')"
      :aria-label="$t('statusBar.versionAria', { version })"
    >
      Version: {{ version }}
    </span>
  </footer>
</template>

<style scoped>
.bypass-btn {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-right: 10px;
  padding: 0 2px;
  background: transparent;
  border: none;
  color: var(--color-ink-dim);
  cursor: pointer;
  transition:
    color 0.15s,
    filter 0.15s;
}
.bypass-btn:hover {
  color: var(--color-ink);
}
/* オンの間は注意色で発光（人の確認が外れている状態）。aria-pressed と文言も併記。 */
.bypass-btn.is-on {
  color: var(--color-warn);
  filter: drop-shadow(0 0 4px var(--color-warn));
}
.run-approval {
  display: flex;
  align-items: center;
  gap: 3px;
  margin-right: 10px;
  color: var(--color-ink-dim);
  transition:
    color 0.15s,
    filter 0.15s;
}
.run-approval select {
  height: 16px;
  padding: 0 2px;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  cursor: pointer;
}
.run-approval:hover {
  color: var(--color-ink);
}
/* 既定以外の間は発光（人の承認が外れている）。自動承認は注意色、承認せずは記録も
   残らないので失敗色。色だけに頼らない — 選んでいるモードの名前が字で出ている。 */
.run-approval.is-auto {
  color: var(--color-warn);
  filter: drop-shadow(0 0 4px var(--color-warn));
}
.run-approval.is-none {
  color: var(--color-fail);
  filter: drop-shadow(0 0 4px var(--color-fail));
}
.stats-btn {
  display: flex;
  align-items: center;
  margin-right: 10px;
  padding: 0 2px;
  background: transparent;
  border: none;
  color: var(--color-ink-dim);
  cursor: pointer;
  transition:
    color 0.15s,
    filter 0.15s;
}
.stats-btn:hover {
  color: var(--color-ink);
}
/* 開いている間は稼働の緑で発光。**色だけに頼らない** — aria-pressed も出している。 */
.stats-btn.is-on {
  color: var(--color-run);
  filter: drop-shadow(0 0 4px var(--color-run));
}
</style>
