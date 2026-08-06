/**
 * 入力欄の `@` パス補完 — トリガ検出と候補の順位付け（Spec 24 P1）。
 *
 * **純関数だけを置く。** IPC も DOM も知らないので、順位の規則が
 * vitest で固定できる（`lib/attachment.ts` / `lib/clock.ts` と同じ形）。
 *
 * # 種別を持つのはこの層
 *
 * [`Candidate`] に `kind` があるのは、**将来サーヴァントの `@` 言及
 * （利用者からの同報）を同じ `@` に載せるため**（Spec 24 D2）。
 * v1 は `"file"` しか作らないが、**候補の合流はここで起きる** —
 * ファイルは IPC、サーヴァントは `state.agents` と供給元が違い、
 * 混ざるのはこの型の配列になる。だから IPC 側には `kind` を置かない。
 */

/** 補完候補 1 件。 */
export interface Candidate {
  /**
   * 本文へ挿す文字列。
   *
   * ファイルなら **work_dir 相対パス**（`specs/01_foo.md`）。
   * 将来サーヴァントを載せるなら表示名になる。
   */
  id: string;
  /**
   * 候補の種別。**v1 は `"file"` だけ。**
   *
   * 将来 `"agent"` が増える席で、増えたときに**照合と表示だけ**が
   * 分岐すればよいように今から持たせてある（Spec 24 D2）。
   */
  kind: "file";
}

/** 入力欄で検出した `@` クエリ。 */
export interface Trigger {
  /** `@` そのものの位置（本文の先頭からの index）。 */
  at: number;
  /** `@` の直後からカーソルまでの文字列。空文字もありうる。 */
  query: string;
}

/** 順位付けされた候補 1 件。 */
export interface Ranked {
  /** 元の候補。 */
  candidate: Candidate;
  /**
   * 一致した位置（表示で強調するため）。
   * ファイル名の中での index で、パス全体の index ではない。
   */
  matchIndex: number;
}

/** 一度に出す候補の上限。これを超える分は捨てる（画面に収まらない）。 */
export const MAX_SUGGESTIONS = 20;

/**
 * カーソル位置から `@` のクエリを検出する。
 *
 * **`@` の直前が単語文字なら発火しない** — メールアドレスや
 * `user@example.com` を打っている途中で候補が出ると邪魔になる。
 * 行頭・空白・括弧の後だけを入口にする。
 *
 * **クエリに空白は含めない。** 空白を打った時点で「もう補完ではない」と
 * 見なして閉じる（`@` を含む普通の文章がそのまま通る = S5 の担保）。
 *
 * @param text 入力欄の全文
 * @param caret カーソル位置（`selectionStart`）
 * @returns 補完を開くべきなら [`Trigger`]、でなければ `null`
 */
export function findTrigger(text: string, caret: number): Trigger | null {
  // カーソルより前だけを見る。後ろに何が書かれていても補完の対象ではない。
  const head = text.slice(0, caret);
  const at = head.lastIndexOf("@");
  if (at < 0) return null;

  // `@` の直前。行頭・空白・開き括弧なら入口として認める。
  const before = at > 0 ? head[at - 1] : "";
  if (before && !/[\s([{「『（]/.test(before)) return null;

  const query = head.slice(at + 1);
  // 空白・改行が入ったら補完は閉じる。
  if (/\s/.test(query)) return null;
  return { at, query };
}

/**
 * 候補を絞り込んで順位付けする。
 *
 * **照合はファイル名（basename）に当てる**（Spec 24 D5）。実測で利用者は
 * ファイル名だけを打って正しい 1 本に当てており、パス全体へ素朴に一致を
 * 掛けると、**深いフォルダ名がたまたま一致した無関係なファイル**が上位に来る。
 *
 * 順位は 4 段。**同点はパスの辞書順**で破る — コア側がソート済みの一覧を
 * 返すので、同じ入力に対して並びが揺れない（Spec 24 P0 実装記録 2）。
 *
 * | 段 | 条件 |
 * |---|---|
 * | 1 | ファイル名が前方一致 |
 * | 2 | ファイル名が部分一致 |
 * | 3 | パス全体が部分一致（ディレクトリ名で当てたとき） |
 * | 4 | 一致しない → 落とす |
 *
 * 大文字小文字は無視する。パスの大小を意識して打つ人は居ない。
 *
 * @param candidates 候補の全件（コアから来た順 = ソート済みを想定）
 * @param query `@` の直後の文字列。空なら先頭から [`MAX_SUGGESTIONS`] 件
 * @param limit 返す上限。既定は [`MAX_SUGGESTIONS`]
 */
export function rankCandidates(
  candidates: Candidate[],
  query: string,
  limit: number = MAX_SUGGESTIONS,
): Ranked[] {
  const needle = query.toLowerCase();

  // クエリが空 = `@` を打った直後。絞り込む材料が無いので先頭から見せる。
  if (!needle) {
    return candidates.slice(0, limit).map((candidate) => ({
      candidate,
      matchIndex: -1,
    }));
  }

  const scored: { rank: number; matchIndex: number; candidate: Candidate }[] = [];
  for (const candidate of candidates) {
    const path = candidate.id.toLowerCase();
    const slash = path.lastIndexOf("/");
    const base = slash < 0 ? path : path.slice(slash + 1);

    const inBase = base.indexOf(needle);
    if (inBase === 0) {
      scored.push({ rank: 1, matchIndex: 0, candidate });
    } else if (inBase > 0) {
      scored.push({ rank: 2, matchIndex: inBase, candidate });
    } else if (path.includes(needle)) {
      // ディレクトリ名で当てた。強調位置はファイル名の中に無いので -1。
      scored.push({ rank: 3, matchIndex: -1, candidate });
    }
  }

  // 段が同じなら、ファイル名の中で早く一致したほうが上。それも同じなら
  // パスの辞書順（= コアが返した並び）。**安定した順序を最後まで残す。**
  scored.sort(
    (a, b) =>
      a.rank - b.rank ||
      a.matchIndex - b.matchIndex ||
      a.candidate.id.localeCompare(b.candidate.id),
  );

  return scored
    .slice(0, limit)
    .map(({ candidate, matchIndex }) => ({ candidate, matchIndex }));
}

/**
 * 候補を確定したときの、入力欄の新しい状態を返す。
 *
 * **`@` は残す。** 消すと本文が `specs/foo.md` になり、**それが補完で
 * 入ったものか人が打ったものか読めなくなる**。`@specs/foo.md` の形なら
 * 会話ログを見た人にも由来が分かる。
 *
 * 確定したら**末尾に空白を 1 つ足す** — 続けて打てるようにし、同時に
 * [`findTrigger`] が閉じる（クエリに空白が入ると `null`）ので、
 * **確定した瞬間にポップアップが閉じる**。閉じる処理を別に書かなくて済む。
 *
 * @returns 新しい本文と、置くべきカーソル位置
 */
export function applyCompletion(
  text: string,
  trigger: Trigger,
  caret: number,
  candidate: Candidate,
): { text: string; caret: number } {
  const head = text.slice(0, trigger.at);
  const tail = text.slice(caret);
  const inserted = `@${candidate.id} `;
  return {
    text: `${head}${inserted}${tail}`,
    caret: trigger.at + inserted.length,
  };
}
