<script setup lang="ts">
/**
 * 発話に添える接地の来歴（Spec 05 Phase 4）。
 *
 * # なぜ吹き出しの外に出すか
 *
 * これは**モデルの発言ではなく、こちらが観測した事実**である。本文と同じ地に
 * 置くと、モデルが書いた「出典」と、実際に返ってきた参照元の区別が消える。
 * その区別を守ることが、この欄の存在理由そのものになる。
 *
 * # なぜ 0 件でも欄を消さないか
 *
 * `sources` が空であることは、情報が無いのではなく **「出典は存在しない」という
 * 判定**である。黙って畳むと、利用者は本文中の URL を出典だと信じる。
 * その URL は、参照元を運ぶ経路を持たないモデルが**作ったもの**でありうる。
 */
import { openUrl } from "@tauri-apps/plugin-opener";

import { isOpenableUrl, sourceLabel, type GroundingView } from "../lib/grounding";

defineProps<{ view: GroundingView }>();

/** 参照元を外部ブラウザで開く。http / https 以外は無視する。 */
function open(uri: string): void {
  if (isOpenableUrl(uri)) void openUrl(uri);
}
</script>

<template>
  <div
    class="mt-1 min-w-0 max-w-full rounded-lg border border-line bg-surface-1 px-2 py-1.5 text-[10px] leading-relaxed text-ink-dim"
  >
    <p class="flex flex-wrap items-baseline gap-1">
      <span class="font-medium text-ink">グラウンディング</span>
      <span>Google 検索</span>
    </p>

    <p v-if="view.queries.length" class="mt-0.5 wrap-anywhere">
      <span class="mr-1">検索語</span>
      <span
        v-for="query in view.queries"
        :key="query"
        class="mr-1 inline-block rounded bg-surface-2 px-1 py-px text-ink"
        >{{ query }}</span
      >
    </p>

    <!-- 本文に URL が無い返答でも文が成立する形にする。「本文中の URL は」と
         書くと、URL を書かなかった返答では存在しないものを指してしまう。 -->
    <p v-if="view.sourcesMissing" class="mt-0.5">
      参照元は返ってきていません（本文に URL があっても、出典としては確認できていません）。
    </p>
    <ul v-else class="mt-0.5 space-y-0.5">
      <li v-for="source in view.sources" :key="source.uri">
        <button
          type="button"
          class="text-left wrap-anywhere text-accent underline decoration-dotted underline-offset-2 hover:decoration-solid"
          :title="source.uri"
          @click="open(source.uri)"
        >
          {{ sourceLabel(source) }}
        </button>
      </li>
    </ul>
  </div>
</template>
