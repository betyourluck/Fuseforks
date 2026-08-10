<script setup lang="ts">
/**
 * 発話に添える**思考の要約**（Spec 33 P3）。
 *
 * # 接地の来歴と枠を分ける（D6）
 *
 * 出典は検証できる外部の指し先、要約は検証できない内部の申告。
 * `GroundingNote` と同じ枠に並べると**後者に前者の信用が乗る**。
 *
 * # 「思考」ではなく「思考の要約」と言う（D5）
 *
 * 返ってくるのは summary であって思考そのものではない — トークン数と字数が
 * 2 桁違う（xAI は最大 1,367 字 / 3,685 トークン、Anthropic は 3,475 字 /
 * 4,096 トークン、Gemini は 1,754 字 / 2,994 トークン）。
 * 「思考を表示します」と書くと、**返っていないものを返っていると言うことになる**。
 *
 * # 既定で畳む
 *
 * 実測で最大 3,475 字あり、しかも英語。開いたまま置くと**答えが埋まる**。
 * 畳むのは D4 の「1 字以上はそのまま出す」に反しない — **中身は落とさず、
 * 開けば全文が読める**。畳んだ状態でも見出しに字数を出すので、
 * 「要約がある」ことと「どのくらいあるか」は開かずに分かる。
 */
import { computed } from "vue";

import type { ThinkingView } from "../lib/thinkingNote";

const props = defineProps<{ view: ThinkingView }>();

/** 周をまたいだ要約が複数あるか（見出しの言い回しが変わる）。 */
const rounds = computed(() => props.view.summaries.length);
</script>

<template>
  <details
    class="mt-1 min-w-0 max-w-full rounded-lg border border-line bg-surface-1 px-2 py-1.5 text-[10px] leading-relaxed text-ink-dim"
  >
    <summary class="cursor-pointer list-none select-none">
      <span class="font-medium text-ink">{{ $t("thinking.heading") }}</span>
      <span class="ml-1 tabular-nums">{{
        $t("thinking.size", { chars: view.chars, rounds })
      }}</span>
    </summary>

    <!-- 原文であることの注記（D7 案 b）。**訳語ではなく事実の記述**なので、
         「思考の要約」と言い切る D5 とも、システムプロンプトを訳さない凍結とも
         衝突しない。3 社とも英語で返るのは実測で、1 社の癖ではない。 -->
    <p class="mt-1 text-ink-dim/80">{{ $t("thinking.verbatim") }}</p>

    <p
      v-for="(one, index) in view.summaries"
      :key="index"
      class="mt-1 whitespace-pre-wrap break-words border-t border-line pt-1"
    >
      {{ one }}
    </p>
  </details>
</template>
