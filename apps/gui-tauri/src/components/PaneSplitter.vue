<script setup lang="ts">
/**
 * ペイン間のつまみ。
 *
 * 移動量を**差分**で通知し、原点をその都度更新する。
 * 開始位置からの絶対差で計算すると、上限に張り付いた後にポインタを戻したとき
 * 「戻した分だけ動く」までに空走りが出て、掴んだ感触が壊れる。
 *
 * `setPointerCapture` を使うので、素早く動かしてつまみの外へ出ても追従する。
 */
import { ref } from "vue";

const props = defineProps<{
  /** `col` = 縦線（左右に動かす） / `row` = 横線（上下に動かす）。 */
  direction: "col" | "row";
  /** 支援技術向けの説明。 */
  label: string;
}>();

const emit = defineEmits<{
  (e: "delta", px: number): void;
  (e: "reset"): void;
}>();

const dragging = ref(false);
let origin = 0;

function position(event: PointerEvent): number {
  return props.direction === "col" ? event.clientX : event.clientY;
}

function onPointerDown(event: PointerEvent): void {
  dragging.value = true;
  origin = position(event);
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function onPointerMove(event: PointerEvent): void {
  if (!dragging.value) return;
  const next = position(event);
  emit("delta", next - origin);
  origin = next;
}

function onPointerUp(event: PointerEvent): void {
  if (!dragging.value) return;
  dragging.value = false;
  (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
}

/** キーボードでも動かせるようにする。1 回 16px。 */
function onKeydown(event: KeyboardEvent): void {
  const forward = props.direction === "col" ? "ArrowRight" : "ArrowDown";
  const backward = props.direction === "col" ? "ArrowLeft" : "ArrowUp";

  if (event.key === forward) emit("delta", 16);
  else if (event.key === backward) emit("delta", -16);
  else return;

  event.preventDefault();
}
</script>

<template>
  <div
    role="separator"
    tabindex="0"
    :aria-label="label"
    :aria-orientation="direction === 'col' ? 'vertical' : 'horizontal'"
    :title="`${label}（ダブルクリックで既定値へ）`"
    class="group relative shrink-0 transition-colors"
    :class="[
      direction === 'col' ? 'cursor-col-resize' : 'cursor-row-resize',
      dragging ? 'bg-accent' : 'bg-line hover:bg-accent',
    ]"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
    @pointercancel="onPointerUp"
    @dblclick="emit('reset')"
    @keydown="onKeydown"
  >
    <!--
      当たり判定を見た目より広げる。1〜2px の線をそのまま掴ませると、
      掴めずに隣のペインをクリックしてしまう。
    -->
    <span
      class="absolute"
      :class="
        direction === 'col'
          ? '-inset-x-1.5 inset-y-0'
          : '-inset-y-1.5 inset-x-0'
      "
    />
  </div>
</template>
