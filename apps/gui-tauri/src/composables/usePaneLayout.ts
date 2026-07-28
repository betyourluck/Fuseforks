/**
 * 3 ペインの寸法。ドラッグで変えられ、次回起動へ引き継がれる。
 *
 * 保存先を localStorage にしているのは、これが**表示の都合**であって
 * オーケストレーターの状態ではないから。`world.json` に混ぜると、
 * 設定を配布したときに相手の画面寸法まで押し付けることになる。
 */

import { reactive, watch } from "vue";

const STORAGE_KEY = "concordia.layout.v1";

/** 各寸法の下限と上限。これを外れると操作不能な画面ができる。 */
const BOUNDS = {
  leftWidth: { min: 220, max: 620 },
  rightWidth: { min: 260, max: 720 },
  logHeight: { min: 120, max: 900 },
} as const;

export interface PaneLayout {
  /** 左ペイン（エージェント一覧）の幅。 */
  leftWidth: number;
  /** 右ペイン（設定）の幅。 */
  rightWidth: number;
  /** 中央下部（会話ログ）の高さ。 */
  logHeight: number;
}

const DEFAULTS: PaneLayout = {
  leftWidth: 320,
  rightWidth: 380,
  logHeight: 280,
};

/** 値を下限・上限へ収める。 */
function clamp(value: number, key: keyof PaneLayout): number {
  const { min, max } = BOUNDS[key];
  return Math.min(Math.max(Math.round(value), min), max);
}

/** 保存済みの寸法を読む。壊れていたら既定値へ落とす。 */
function load(): PaneLayout {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };

    const parsed = JSON.parse(raw) as Partial<PaneLayout>;
    return {
      leftWidth: clamp(parsed.leftWidth ?? DEFAULTS.leftWidth, "leftWidth"),
      rightWidth: clamp(parsed.rightWidth ?? DEFAULTS.rightWidth, "rightWidth"),
      logHeight: clamp(parsed.logHeight ?? DEFAULTS.logHeight, "logHeight"),
    };
  } catch {
    // 壊れた保存値で画面が開けなくなるほうが害が大きい。
    return { ...DEFAULTS };
  }
}

const layout = reactive<PaneLayout>(load());

watch(layout, (next) => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 保存できなくても操作は続けられる。握り潰してよい唯一の経路。
  }
});

export function usePaneLayout() {
  return {
    layout,

    /**
     * つまみのドラッグ量を寸法へ反映する。
     *
     * `sign` は「つまみを右（下）へ動かしたときに値が増えるか」。
     * 右ペインは左端につまみがあるので、右へ動かすと幅は縮む。
     */
    resize(key: keyof PaneLayout, deltaPx: number, sign: 1 | -1 = 1): void {
      layout[key] = clamp(layout[key] + deltaPx * sign, key);
    },

    /** 既定値へ戻す。ドラッグで見失ったときの避難路。 */
    reset(): void {
      Object.assign(layout, DEFAULTS);
    },
  };
}
