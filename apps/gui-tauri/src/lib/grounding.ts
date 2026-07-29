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

import type { AgentMessage, GroundingSource } from "../types";

/** 表示に必要な形へ畳んだ接地の来歴。 */
export interface GroundingView {
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
 * 参照元の表示ラベル。表題が空なら URL のホスト名で代用する。
 *
 * 空文字のリンクは押せる場所が消えるので、必ず何かを返す。
 */
export function sourceLabel(source: GroundingSource): string {
  if (source.title.trim()) return source.title;
  try {
    return new URL(source.uri).hostname;
  } catch {
    return source.uri;
  }
}
