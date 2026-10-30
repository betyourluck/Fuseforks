/**
 * 入力欄の添付の列（Spec 73 D7）。
 *
 * 1 発話に複数件の添付を載せ、過去の添付を ID だけで渡し直す。**上限の値はコアと同じ**
 * （`ATTACHMENT_MAX_PER_MESSAGE` / `ATTACHMENT_MAX_MESSAGE_BYTES`）で、判定の正はコアの
 * 送信入口にある。ここで同じ上限を見るのは、**取り込む前に断る**ため — 6 件目を貼った後で
 * 送信が拒まれると、どれを外せばよいかが送るまで分からない。
 *
 * 画面の状態（DOM・IPC）は持たない。部品は `ChatInput.vue`。
 */

import type { PendingAttachment } from "./attachment";
import type { AttachmentKind } from "./carries";
import type { Attachment } from "../types";

/** 1 発話の件数の上限。コアの `ATTACHMENT_MAX_PER_MESSAGE` と同じ値。 */
export const MAX_ATTACHMENTS = 5;

/** 1 発話の合計バイトの上限。コアの `ATTACHMENT_MAX_MESSAGE_BYTES` と同じ値。 */
export const MAX_MESSAGE_BYTES = 12 * 1024 * 1024;

/**
 * 入力欄に載っている添付 1 件。
 *
 * - `upload` = いま貼った・選んだファイル（変換済み）
 * - `ref` = 会話ペインの「入力欄へ」で渡し直す過去の添付。**運ぶのは ID だけ**で、
 *   `bytes` は表示と合計のために読んだファイルの長さ（コアは自分で測り直す）
 */
export type DraftAttachment =
  | { source: "upload"; key: string; file: PendingAttachment }
  | { source: "ref"; key: string; ref: Attachment; kind: AttachmentKind; bytes: number };

/** 取り込みを断った理由。辞書キー（`chatInput.attachLimit.*`）に 1 対 1 で対応する。 */
export type AttachLimit = "count" | "bytes" | "duplicate";

/** 再添付の鍵。同じ過去の添付を 2 回載せない（コアも ID で重複を落とす）。 */
export function refKey(id: string): string {
  return `ref:${id}`;
}

/** 1 件の大きさ（bytes）。 */
export function bytesOf(item: DraftAttachment): number {
  return item.source === "upload" ? item.file.bytes : item.bytes;
}

/** 1 件の種別。 */
export function kindOf(item: DraftAttachment): AttachmentKind {
  return item.source === "upload" ? item.file.kind : item.kind;
}

/** 列の合計バイト。 */
export function totalBytes(items: readonly DraftAttachment[]): number {
  return items.reduce((sum, item) => sum + bytesOf(item), 0);
}

/**
 * あと何件載せられるか。
 *
 * **貼り付けた複数ファイルは、変換の前にこの数で切る** — 変換は重いので、断る分まで
 * 変換してから捨てない。切った残りは理由つきで知らせる（黙って捨てない）。
 */
export function roomFor(items: readonly DraftAttachment[]): number {
  return Math.max(0, MAX_ATTACHMENTS - items.length);
}

/**
 * 1 件を列へ足せるか。足せないなら理由を返す。
 *
 * 判定の順は コアの送信入口と同じ（件数 → 合計）。重複は再添付だけで起きる
 * （新しく貼ったものは毎回別の鍵を持つ）。
 */
export function admit(
  items: readonly DraftAttachment[],
  incoming: DraftAttachment,
): AttachLimit | null {
  if (items.some((item) => item.key === incoming.key)) return "duplicate";
  if (items.length >= MAX_ATTACHMENTS) return "count";
  if (totalBytes(items) + bytesOf(incoming) > MAX_MESSAGE_BYTES) return "bytes";
  return null;
}

/**
 * 列が上限を超えているか（送信を止める理由）。
 *
 * 普通の操作では `admit` が手前で断るので起きない。起きるのは**送信に失敗した下書きを
 * 戻したとき**だけ — 待っている間に貼られた分と合わせて上限を超えうる。どちらかを
 * 黙って捨てず、超えていることを出して利用者に外してもらう。
 */
export function overLimit(items: readonly DraftAttachment[]): AttachLimit | null {
  if (items.length > MAX_ATTACHMENTS) return "count";
  if (totalBytes(items) > MAX_MESSAGE_BYTES) return "bytes";
  return null;
}

/**
 * 送信する形へ割る。新しく貼ったものは本体ごと、再添付は ID だけ。
 *
 * 発話の中の並びはコアが決める（新規 → 再添付。S73-5）ので、ここでは割るだけ。
 */
export function splitForSend(items: readonly DraftAttachment[]): {
  uploads: PendingAttachment[];
  refIds: string[];
} {
  const uploads: PendingAttachment[] = [];
  const refIds: string[] = [];
  for (const item of items) {
    if (item.source === "upload") uploads.push(item.file);
    else refIds.push(item.ref.id);
  }
  return { uploads, refIds };
}

/** 列に含まれる種別（重複なし・`AttachmentKind` の宣言順）。 */
export function kindsIn(items: readonly DraftAttachment[]): AttachmentKind[] {
  const order: AttachmentKind[] = ["image", "audio", "video", "pdf"];
  const present = new Set(items.map(kindOf));
  return order.filter((kind) => present.has(kind));
}

/**
 * MB 表記（小数 1 桁）。合計の行「3 件・4.2 MB / 12 MB」に使う。
 *
 * **言語に追従させない** — 数字の書式は版番号や時計と同じく識別子の側で、上限の
 * 「12 MB」と同じ形に揃っていないと、どこまで載るかが読み比べられない。
 */
export function formatMegabytes(bytes: number): string {
  return (bytes / (1024 * 1024)).toFixed(1);
}
