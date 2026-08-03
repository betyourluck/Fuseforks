/**
 * 役職の表示名を引く（Spec 14 の `role_contract` 凍結 5）。
 *
 * # なぜ 1 実装に寄せるか
 *
 * `role_id` から表示名を引く場所はカードのバッジ・地図のバッジ・顔ぶれの行の
 * **3 箇所**あり、**引けなかったときの扱いを揃える必要がある**。3 箇所で規則が
 * 分かれると「画面のバッジは消えたのにプロンプトには `[不明]` が残る」のような
 * 食い違いが生まれる。`world::exchange_pair` を 1 つにした判断（Spec 12 P1）と
 * 同じ形で、実装をここ 1 箇所に置く。
 *
 * # 引けないときは表示ごと省く
 *
 * **`[不明]` とは書かない。** 存在しない役を出しても判断材料にならず、
 * 顔ぶれでは毎ターンぶんのトークンを払うだけになる。
 *
 * 役職が消えてもサーヴァントの動作は変わらない（設定は作成時にコピー済み）ので、
 * 「壊れている」と読ませる表示を出すのはむしろ嘘になる。
 */
import type { Role, RoleColor, RoleId } from "../types";

/** バッジ 1 つぶんの表示材料。 */
export interface RoleBadge {
  /** 表示名。 */
  name: string;
  /**
   * 枠線と字の色（CSS の値）。色なしの役職では `undefined` で、
   * 呼び出し側は既定の枠線・字色をそのまま使う。
   *
   * **値は `style.css` の `--color-role-*` を参照するだけ**で、ここで生の色を
   * 組み立てない（配色を `@theme` の外へ漏らさない規律）。
   */
  color?: string;
}

/**
 * `roleId` のバッジ表示。引けなければ `null`（呼び出し側はバッジごと描かない）。
 *
 * **名前と色を 1 回で返す**のは、2 つの関数に分けると片方だけ呼ぶ実装が生えて
 * 「名前は出るが色が付かない」場所ができるため。3 箇所（カード・地図・顔ぶれ）が
 * 同じ結果を見ることが凍結 5 の要件。
 *
 * @param roleId 個体が持つ役職 id。`null` = 役職なし
 * @param roles  登録済みの役職一覧（`state.roles`）
 */
export function roleBadge(roleId: RoleId | null, roles: Role[]): RoleBadge | null {
  if (!roleId) return null;
  const role = roles.find((r) => r.id === roleId);
  if (!role) return null;
  return { name: role.name, color: cssColorOf(role.color) };
}

/** 閉じた列挙 → CSS 変数。列挙を増やしたら `style.css` の `--color-role-*` も足す。 */
function cssColorOf(color: RoleColor | null | undefined): string | undefined {
  return color ? `var(--color-role-${color})` : undefined;
}

/**
 * `roleId` の表示名だけ。顔ぶれのような**色を持てない場所**で使う。
 *
 * プロンプトへ載るのは名前だけなので（凍結 6）、色を組み立てる必要がない。
 */
export function roleLabel(roleId: RoleId | null, roles: Role[]): string | null {
  return roleBadge(roleId, roles)?.name ?? null;
}
