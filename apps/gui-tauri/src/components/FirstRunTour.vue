<script setup lang="ts">
/**
 * 初回起動のナビゲーション（2026-09-13 利用者要望）。移植元は AppPromoVideo / Lorekeel の
 * `FirstRunTour.vue`。
 *
 * **手順を教えるだけ** — 案内の中から登録・作成・起動はさせない（移植元と同じ流儀。
 * 案内が対象を押す形にすると、対象側の部品が案内の状態を知る必要が生まれる）。
 * 挨拶の幕のあと、**実物の UI 要素**をスポットライトが順に照らし、横に番号つきのカードが
 * 出る。対象は `data-tour="…"` 属性で印を付けた要素で、この部品は `querySelectorAll` で
 * 測るだけ = 対象側の部品はこの部品を知らない。
 *
 * 印が 2 つある歩が 2 つ（`kizuna` = 一覧の本体 + 地図のキャンバス / `titlebar` = 「条例」と
 * 「システム設定」）。矩形を束ねて間を覆う（`spotlightUnion`）。**包む div を足さない** —
 * 足すと余白の出方が変わり、タイトルバーは `data-tauri-drag-region` の領域も動く。
 *
 * 依存ゼロ・CSS のトランジションだけ・動的 import で初回にしか読まれない（App.vue）。
 * 色は全部 `style.css` のトークンから引く（テーマで変わる）。
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import {
  TOUR_DONE_KEY,
  TOUR_STEPS,
  placeCard,
  spotlightBox,
  spotlightUnion,
  type Box,
  type CardPlacement,
  type TourStep,
} from "../lib/tour";

const { t } = useI18n();
const emit = defineEmits<{ (e: "close"): void }>();

/**
 * 歩ごとの辞書の鍵。**字面で書く** — `` t(`tour.steps.${step}.title`) `` と組み立てると、
 * 辞書から鍵を消しても grep に掛からず、実機で `tour.steps.models.title` という
 * 生の鍵が画面に出るまで気づけない。`tourTargets.test.ts` がこの表と辞書を突き合わせる。
 */
const STEP_TEXT: Record<TourStep, { title: string; body: string }> = {
  models: { title: "tour.steps.models.title", body: "tour.steps.models.body" },
  servant: { title: "tour.steps.servant.title", body: "tour.steps.servant.body" },
  kizuna: { title: "tour.steps.kizuna.title", body: "tour.steps.kizuna.body" },
  start: { title: "tour.steps.start.title", body: "tour.steps.start.body" },
  chat: { title: "tour.steps.chat.title", body: "tour.steps.chat.body" },
  titlebar: { title: "tour.steps.titlebar.title", body: "tour.steps.titlebar.body" },
  board: { title: "tour.steps.board.title", body: "tour.steps.board.body" },
  stats: { title: "tour.steps.stats.title", body: "tour.steps.stats.body" },
  workdir: { title: "tour.steps.workdir.title", body: "tour.steps.workdir.body" },
};

const phase = ref<"welcome" | "tour">("welcome");
const idx = ref(0);
const step = computed(() => TOUR_STEPS[idx.value]);
const total = TOUR_STEPS.length;

const spot = ref<Box>({ left: 0, top: 0, width: 0, height: 0 });
const card = ref<CardPlacement>({ left: 0, top: 0, side: "below", arrow: 18 });
const cardEl = ref<HTMLElement | null>(null);
const measured = ref(false);

/** カードの幅は CSS（22rem / 最大 92vw）から決まる。実寸を測れない瞬間もこの値で置ける。 */
function cardWidthFromCss(vpWidth: number): number {
  const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
  return Math.min(22 * rem, vpWidth * 0.92);
}

/** `sizeEl` = 寸法を測るカード要素。Transition の enter 中は ref がまだ新しい要素を指していない。 */
function measure(sizeEl?: HTMLElement | null): void {
  const els = Array.from(
    document.querySelectorAll<HTMLElement>(`[data-tour="${step.value}"]`),
  );
  const vp = { width: window.innerWidth, height: window.innerHeight };
  const union = spotlightUnion(els.map((el) => el.getBoundingClientRect()));
  // 対象が見つからない = レイアウトが変わった。それでも案内は続けられるよう画面中央に小さく置く。
  spot.value = union
    ? spotlightBox(union, 8)
    : { left: vp.width / 2 - 24, top: vp.height / 2 - 24, width: 48, height: 48 };

  const src = sizeEl ?? cardEl.value;
  const size =
    src && src.offsetWidth > 0
      ? { width: src.offsetWidth, height: src.offsetHeight }
      : { width: cardWidthFromCss(vp.width), height: 190 };
  card.value = placeCard(spot.value, vp, size, 14);
  measured.value = true;
}

async function remeasure(): Promise<void> {
  await nextTick();
  measure();
  // カードの中身（文言の長さ）で高さが変わるので、描画後にもう一度置き直す。
  await nextTick();
  measure();
}

function begin(): void {
  phase.value = "tour";
  idx.value = 0;
  void remeasure();
}
function next(): void {
  if (idx.value + 1 >= total) return finish();
  idx.value += 1;
  void remeasure();
}
function back(): void {
  if (idx.value === 0) return;
  idx.value -= 1;
  void remeasure();
}
function finish(): void {
  try {
    localStorage.setItem(TOUR_DONE_KEY, "1");
  } catch {
    /* storage が書けない環境でも案内自体は閉じる */
  }
  emit("close");
}

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    finish();
  } else if (e.key === "Enter" || e.key === "ArrowRight") {
    e.preventDefault();
    if (phase.value === "welcome") begin();
    else next();
  } else if (e.key === "ArrowLeft" && phase.value === "tour") {
    e.preventDefault();
    back();
  }
}
function onResize(): void {
  measure();
}
onMounted(() => {
  window.addEventListener("keydown", onKey);
  window.addEventListener("resize", onResize);
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKey);
  window.removeEventListener("resize", onResize);
});
watch(phase, () => void remeasure());

const spotStyle = computed(() => ({
  left: `${spot.value.left}px`,
  top: `${spot.value.top}px`,
  width: `${spot.value.width}px`,
  height: `${spot.value.height}px`,
}));
const cardStyle = computed(() => ({
  left: `${card.value.left}px`,
  top: `${card.value.top}px`,
  "--arrow": `${card.value.arrow}px`,
}));
</script>

<template>
  <div class="tour-root" role="dialog" aria-modal="true" :aria-label="t('tour.welcomeTitle')">
    <!-- ===== 挨拶の幕 ===== -->
    <Transition name="tour-fade">
      <div v-if="phase === 'welcome'" class="tour-welcome" @click.self="begin">
        <div class="tour-welcome-card">
          <p class="tour-brand"><span class="outcasts-word">Outcasts</span> Fuseforks</p>
          <h2 class="tour-title">{{ t("tour.welcomeTitle") }}</h2>
          <p class="tour-lead">{{ t("tour.welcomeLead") }}</p>
          <ol class="tour-minis">
            <li
              v-for="(s, i) in TOUR_STEPS"
              :key="s"
              class="tour-mini"
              :style="{ '--d': `${0.4 + i * 0.08}s` }"
            >
              <span class="tour-num">{{ i + 1 }}</span>
              <span class="tour-mini-text">{{ t(STEP_TEXT[s].title) }}</span>
            </li>
          </ol>
          <div class="tour-actions">
            <button class="tour-btn primary" autofocus @click="begin">{{ t("tour.start") }}</button>
            <button class="tour-btn" @click="finish">{{ t("tour.skip") }}</button>
          </div>
          <p class="tour-keys">{{ t("tour.keys") }}</p>
        </div>
      </div>
    </Transition>

    <!-- ===== コーチマーク ===== -->
    <template v-if="phase === 'tour'">
      <!-- スポットライト: 巨大な box-shadow で周囲を暗くし、対象だけ素の UI が見える。
           left/top/width/height に遷移を掛ける = 対象の間を滑って移動する。 -->
      <div class="tour-spot" :class="{ ready: measured }" :style="spotStyle" @click.stop="next" />
      <!-- 暗幕のどこを押しても次へ（対象以外は操作させない） -->
      <div class="tour-veil" @click="next" />

      <!-- out-in なので新しいカードは古いのが消えてから入る = 入った要素の実寸で置き直す。 -->
      <Transition name="tour-card" mode="out-in" @enter="(el) => measure(el as HTMLElement)">
        <div
          :key="step"
          ref="cardEl"
          class="tour-card"
          :class="`side-${card.side}`"
          :style="cardStyle"
          @click.stop
        >
          <div class="tour-card-head">
            <span class="tour-num lg">{{ idx + 1 }}</span>
            <h3>{{ t(STEP_TEXT[step].title) }}</h3>
            <span class="tour-count">{{ idx + 1 }} / {{ total }}</span>
          </div>
          <p class="tour-body">{{ t(STEP_TEXT[step].body) }}</p>
          <div class="tour-actions">
            <button v-if="idx + 1 < total" class="tour-btn primary small" @click="next">
              {{ t("tour.next") }}
            </button>
            <button v-else class="tour-btn primary small" @click="finish">
              {{ t("tour.finish") }}
            </button>
            <button v-if="idx > 0" class="tour-btn small" @click="back">{{ t("tour.back") }}</button>
            <button class="tour-btn small tour-skip" @click="finish">{{ t("tour.skip") }}</button>
          </div>
          <div class="tour-dots">
            <span v-for="(s, i) in TOUR_STEPS" :key="s" class="tour-dot" :class="{ on: i <= idx }" />
          </div>
        </div>
      </Transition>
    </template>
  </div>
</template>

<style scoped>
/*
 * z は 55 — ダイアログ（40）より上、確認ダイアログとトースト（60）より下。
 * 案内が出るのは空の村の初回か、設定から呼び直したときで、どちらもダイアログは
 * 閉じている。確認は案内の上に来なければ「閉じる」の確認が案内の裏に隠れる。
 */
.tour-root {
  position: fixed;
  inset: 0;
  z-index: 55;
}

/* ===== 挨拶の幕 ===== */
.tour-welcome {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background:
    radial-gradient(
      ellipse 70% 55% at 50% 110%,
      color-mix(in oklab, var(--color-accent) 22%, transparent),
      transparent 70%
    ),
    color-mix(in oklab, var(--color-surface-0) 94%, transparent);
}
.tour-welcome-card {
  width: 36rem;
  max-width: 92vw;
  padding: 28px 28px 22px;
  text-align: center;
  border: 1px solid var(--color-line);
  border-radius: 12px;
  background: var(--color-surface-1);
  box-shadow: 0 24px 60px color-mix(in oklab, var(--color-surface-0) 70%, transparent);
  animation: tour-in 0.5s cubic-bezier(0.2, 0.8, 0.2, 1) both;
}
/* ワードマークの色と光は `style.css` の `.outcasts-word`（TitleBar と共有）。ここは大きさだけ。 */
.tour-brand {
  margin: 0;
  font-size: 20px;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: var(--color-ink);
}
.tour-title {
  margin: 12px 0 0;
  font-size: 17px;
  font-weight: 700;
  color: var(--color-ink);
}
.tour-lead {
  margin: 8px 0 0;
  color: var(--color-ink-dim);
  font-size: 12px;
  line-height: 1.7;
}
/* 9 歩なので 3 × 3。4 列だと最後の行が 1 つだけ残って欠けて見える。 */
.tour-minis {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
  margin: 18px 0 0;
  padding: 0;
  list-style: none;
  text-align: left;
}
.tour-mini {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border: 1px solid var(--color-line);
  border-radius: 8px;
  background: var(--color-surface-0);
  animation: tour-rise 0.5s cubic-bezier(0.2, 0.8, 0.2, 1) both;
  animation-delay: var(--d, 0s);
}
.tour-mini-text {
  font-size: 12px;
  line-height: 1.4;
  color: var(--color-ink);
}
.tour-num {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: 9999px;
  background: var(--color-accent);
  color: var(--color-surface-0);
  font-size: 11px;
  font-weight: 700;
}
.tour-num.lg {
  width: 26px;
  height: 26px;
  font-size: 12px;
}
.tour-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 18px;
  justify-content: center;
}
.tour-keys {
  margin: 10px 0 0;
  font-size: 11px;
  color: var(--color-ink-dim);
  opacity: 0.8;
}

/* ボタンは村の作法（`bg-accent text-surface-0` / `border-line text-ink-dim`）を素の CSS で。 */
.tour-btn {
  border: 1px solid var(--color-line);
  border-radius: 6px;
  padding: 6px 14px;
  font-size: 12px;
  font-weight: 500;
  color: var(--color-ink-dim);
  background: transparent;
  cursor: pointer;
  transition: color 0.15s ease, border-color 0.15s ease;
}
.tour-btn:hover {
  color: var(--color-ink);
}
.tour-btn.primary {
  border-color: var(--color-accent);
  background: var(--color-accent);
  color: var(--color-surface-0);
}
.tour-btn.primary:hover {
  filter: brightness(1.08);
}
.tour-btn.small {
  padding: 4px 10px;
}

/* ===== コーチマーク ===== */
.tour-veil {
  position: absolute;
  inset: 0;
}
.tour-spot {
  position: fixed;
  border-radius: 8px;
  /* 巨大な box-shadow で周囲だけ暗くする = 対象は素の UI が見える。暗幕の色は
     ダイアログの `bg-scrim` と同じトークン（テーマで変わる）。 */
  box-shadow:
    0 0 0 200vmax var(--color-scrim),
    0 0 0 2px var(--color-accent),
    0 0 22px 4px color-mix(in oklab, var(--color-accent) 40%, transparent);
  opacity: 0;
  cursor: pointer;
  transition:
    opacity 0.3s ease,
    left 0.5s cubic-bezier(0.2, 0.8, 0.2, 1),
    top 0.5s cubic-bezier(0.2, 0.8, 0.2, 1),
    width 0.5s cubic-bezier(0.2, 0.8, 0.2, 1),
    height 0.5s cubic-bezier(0.2, 0.8, 0.2, 1);
}
.tour-spot.ready {
  opacity: 1;
}
.tour-card {
  position: fixed;
  width: 22rem;
  max-width: 92vw;
  padding: 14px 16px;
  border: 1px solid var(--color-line);
  border-radius: 10px;
  background: var(--color-surface-1);
  box-shadow: 0 16px 40px color-mix(in oklab, var(--color-surface-0) 70%, transparent);
}
/* 対象を指す三角。カードを画面内へ寄せても --arrow が対象の中心を指し続ける。 */
.tour-card::before {
  content: "";
  position: absolute;
  width: 10px;
  height: 10px;
  background: var(--color-surface-1);
  border: 1px solid var(--color-line);
  transform: rotate(45deg);
}
.tour-card.side-below::before {
  top: -6px;
  left: var(--arrow, 18px);
  border-right: none;
  border-bottom: none;
}
.tour-card.side-above::before {
  bottom: -6px;
  left: var(--arrow, 18px);
  border-left: none;
  border-top: none;
}
.tour-card.side-right::before {
  left: -6px;
  top: var(--arrow, 18px);
  border-right: none;
  border-top: none;
}
.tour-card.side-left::before {
  right: -6px;
  top: var(--arrow, 18px);
  border-left: none;
  border-bottom: none;
}
.tour-card-head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.tour-card-head h3 {
  margin: 0;
  font-size: 13px;
  font-weight: 700;
  line-height: 1.3;
  color: var(--color-ink);
}
.tour-count {
  margin-left: auto;
  font-size: 11px;
  color: var(--color-ink-dim);
  font-variant-numeric: tabular-nums;
}
.tour-body {
  margin: 10px 0 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-ink);
}
.tour-card .tour-actions {
  justify-content: flex-start;
  margin-top: 14px;
}
.tour-skip {
  margin-left: auto;
}
.tour-dots {
  display: flex;
  gap: 5px;
  margin-top: 12px;
}
.tour-dot {
  width: 16px;
  height: 3px;
  border-radius: 9999px;
  background: var(--color-line);
}
.tour-dot.on {
  background: var(--color-accent);
}

/* ===== 出入り ===== */
@keyframes tour-in {
  from {
    opacity: 0;
    transform: translateY(12px) scale(0.98);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@keyframes tour-rise {
  from {
    opacity: 0;
    transform: translateY(8px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
.tour-fade-leave-active {
  transition: opacity 0.3s ease;
}
.tour-fade-leave-to {
  opacity: 0;
}
.tour-card-enter-active,
.tour-card-leave-active {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}
.tour-card-enter-from,
.tour-card-leave-to {
  opacity: 0;
  transform: translateY(6px);
}
</style>
