<script setup lang="ts">
/**
 * フッターのステータスバー。今は日付と時刻だけを出す。
 *
 * # なぜ置くか
 *
 * これは管理ツールなので、**画面を撮ったときに「いつの状態か」が写っている**
 * 必要がある（利用者要望 2026-08-03）。撮った後で思い出せない情報は、
 * スクリーンショットを証拠として使えなくする。
 *
 * 形式を `concordia.log` の行頭（`diag.rs` の `%Y-%m-%d %H:%M:%S%.3f`）と
 * 揃えてあるので、**撮った画面の時刻からログの該当行を目で引ける**。
 * 整形の詳細と、言語に追従させない理由は `lib/clock.ts` に書いた。
 *
 * # 更新の刻み
 *
 * 1 秒。毎回 `new Date()` を読み直すので、`setInterval` の遅れは累積しない
 * （スリープ復帰後もその場で正しい時刻に戻る）。秒まで出すのは、ログ側が
 * ミリ秒精度で、分までだと突き合わせの候補が 1 分ぶん残るため。
 *
 * # 左側が空いていること
 *
 * 意図的に空けてある。ステータスバーは「常に見えている細い帯」で、
 * 常駐させるだけの価値がある情報しか置かない器。増やすときは
 * **常に見る必要があるか**を毎回問う（設定ダイアログの左メニューと同じ規律 —
 * 器を作ることと、器を埋めることは別）。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { formatClock } from "../lib/clock";

/** 現在時刻。1 秒ごとにティッカーが差し替える。 */
const now = ref(new Date());
const clock = computed(() => formatClock(now.value));

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
    class="flex h-[22px] shrink-0 select-none items-center justify-end border-t border-line bg-surface-1 px-3 text-[11px] text-ink-dim"
  >
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
  </footer>
</template>
