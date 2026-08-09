/**
 * 接地の来歴の表示規則（Spec 05 Phase 4）。
 *
 * # なぜ「参照元 0 件」を無表示にしないか
 *
 * `sources` が空であることは、情報が無いのではなく **「出典は存在しない」という
 * 判定そのもの**である。検索は起きた（`queries` がある）のに参照元が返って
 * こなかった状態を黙って畳むと、利用者は本文中の URL を出典だと信じる。
 * その本文の URL は、経路を持たないモデルが**作ったもの**でありうる
 * （2026-07-29 に実際に、ドメインのルート URL へ記事の見出しを添えた偽の引用が
 * 返っている）。だから 0 件は 0 件として書く。
 *
 * ChatPanel から切り出した純関数。表示規則はコンポーネントの外でテストする。
 */

import type { AgentMessage, GroundingEngine, GroundingSource } from "../types";

/** 表示に必要な形へ畳んだ接地の来歴。 */
export interface GroundingView {
  /**
   * どの機構が接地したか。表示名はここから辞書で引く（Spec 31 D5）。
   *
   * **欄を持たない古い発話は `google` として読む** — `engine` は Spec 31 で
   * 足した欄で、それ以前の記録はすべて Spec 05 の Google 検索由来。
   * コア側の serde 既定と同じ向きに揃えてある。
   */
  engine: GroundingEngine;
  /** モデルが実際に投げた検索語。 */
  queries: string[];
  /** 参照元。`sourcesMissing` が真ならここは空。 */
  sources: GroundingSource[];
  /** 検索は起きたが参照元が返ってこなかった状態。「出典なし」と明記する根拠。 */
  sourcesMissing: boolean;
}

/**
 * 発話から表示用の来歴を作る。接地が起きていなければ `null`。
 *
 * `null` と「空の来歴」を型で分けるのは、前者が**接地していない発話**
 * （大多数）で、後者が**接地したが出典が無い発話**だから。画面に出す文言が
 * 正反対になるので、同じ値へ畳んではいけない。
 */
export function groundingView(message: AgentMessage): GroundingView | null {
  const grounding = message.grounding;
  if (!grounding) return null;

  const queries = grounding.queries ?? [];
  const sources = grounding.sources ?? [];
  if (!queries.length && !sources.length) return null;

  return {
    engine: grounding.engine ?? "google",
    queries,
    sources,
    sourcesMissing: sources.length === 0,
  };
}

/**
 * 外部ブラウザで開いてよい URL か。
 *
 * 許すのは http / https のみ。来歴の URL は Google の `groundingMetadata`
 * 由来だが、**モデルの出力を経由して届く値を信頼しない**規則は本文中の
 * リンク（ChatPanel の `onMarkdownClick`）と揃える。`javascript:` や
 * `file:` を webview の opener へ渡さないための入口の関門。
 */
export function isOpenableUrl(uri: string): boolean {
  return /^https?:\/\//i.test(uri);
}

/**
 * 参照元の表示ラベル。表題が空なら URL から作る。
 *
 * 空文字のリンクは押せる場所が消えるので、必ず何かを返す。
 *
 * # なぜホスト名だけでは足りないか
 *
 * xAI の X 検索は**同じホストの投稿を数十件**返す（実機で 45 件。すべて
 * `x.com`）。ホスト名で代用すると、並んだ全部が同じ文字列になり
 * **リンクの区別が付かない**。X の status URL は投稿者を経路に持っているので、
 * そこを取って `@handle` を出す。
 *
 * **投稿者名は URL から読んだ値であって、本人性の検証ではない。**
 * それでも「どのアカウントの投稿か」は URL の一部として確実に正しく、
 * 45 個の同じ文字列よりは判断材料になる。
 */
export function sourceLabel(source: GroundingSource): string {
  if (source.title.trim()) return source.title;
  try {
    const url = new URL(source.uri);
    const handle = xHandle(url);
    return handle ?? url.hostname;
  } catch {
    return source.uri;
  }
}

/**
 * 参照元に添えるアイコンの種別。無ければ `null`。
 *
 * **返すのは X の投稿だけ。** プロフィールや検索結果の x.com には付けない —
 * それらのラベルはホスト名（`x.com`）のままなので、アイコンを添えると
 * `[X] x.com` になって同じことを 2 回言う。アイコンが意味を足すのは、
 * ラベルが `@handle` になっていて**ホストがどこか読めない**ときだけ。
 */
export function sourceIcon(source: GroundingSource): "x" | null {
  if (source.title.trim()) return null;
  try {
    return xHandle(new URL(source.uri)) ? "x" : null;
  } catch {
    return null;
  }
}

/** X の投稿 URL から `@handle` を取る。投稿 URL でなければ `null`。 */
function xHandle(url: URL): string | null {
  if (!/^(www\.)?(x|twitter)\.com$/i.test(url.hostname)) return null;
  // /{handle}/status/{id} だけを投稿と見なす。プロフィールや検索結果の URL を
  // 投稿として扱うと、押した先が想像と違うものになる。
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts.length < 3 || parts[1].toLowerCase() !== "status") return null;
  return `@${parts[0]}`;
}
