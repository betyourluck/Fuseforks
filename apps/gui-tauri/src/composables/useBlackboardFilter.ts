/**
 * 黒板タブの持ち主の絞り込み（判定は `lib/blackboardFilter`）。
 *
 * **部品の外（モジュール単位）に持つが、保存はしない。** 黒板タブは `v-if` で切り替わる
 * ので、部品の `ref` だと作業状況を見て戻るたびに絞り込みが外れる（畳みと同じ理由）。
 * 一方で `localStorage` に残すと、次に起動したとき**黒板が理由なく一部しか見えない**
 * 状態から始まる。畳みは見えていたものを隠すだけだが、絞り込みは付箋そのものを
 * 画面から消すので、再起動をまたがせない。
 */

import { ref } from "vue";

import type { OwnerFilter } from "../lib/blackboardFilter";

const filter = ref<OwnerFilter>("all");

export function useBlackboardFilter() {
  return filter;
}
