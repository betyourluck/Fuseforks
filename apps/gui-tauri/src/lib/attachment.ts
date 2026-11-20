/**
 * 添付 — フロント側の判定・変換・上限（Spec 23 = 画像 / Spec 36 = 多モーダル）。
 *
 * 「フロントは常に WebP を作る」（D3）の実装。重い処理（デコード・縮小・
 * エンコード）は WebWorker（`workers/imageConvert.ts`）で行い、
 * メインスレッドを止めない。このファイルは**純関数と worker の窓口**だけを持ち、
 * 純関数は vitest で固定する。
 *
 * 上限は 2 段（D5）:
 * - 元ファイル 10MB — **読み込みの前**に見る（`file.size`。#19）。20MB の HEIC を
 *   読んでデコードしてから断ると、断るまでの間メモリと時間を払う
 * - 変換後 2MB — コア側（`validate_attachment`）と同じ値。フロントで先に
 *   見るのは、IPC を往復してから拒否されるより手前で分かるほうが速いだけで、
 *   **門はコア側が本体**（IPC を直接叩く経路はコアが塞ぐ）
 */
import { KIND_MAX_BYTES, type AttachmentKind } from "./carries";

export type { AttachmentKind };

/** 画像の元ファイルの上限（bytes）。デコード前の門。 */
export const MAX_SOURCE_BYTES = 10 * 1024 * 1024;

/**
 * 種別の判定に読む先頭のバイト数（#19）。`detectKind` が見るのは先頭 12 バイトまで。
 */
export const HEAD_BYTES = 16;

/** 変換後（WebP）の上限（bytes）。コアの `ATTACHMENT_IMAGE_MAX_BYTES` と同じ値。 */
export const MAX_CONVERTED_BYTES = 2 * 1024 * 1024;

/** 長辺の上限（px）。コアの `ATTACHMENT_IMAGE_MAX_EDGE_PX` と同じ値。 */
export const MAX_EDGE_PX = 1568;

/** WebP の品質（lossy）。スクリーンショットの文字が読める範囲で小さく。 */
export const WEBP_QUALITY = 0.85;

/**
 * 送信を待っている添付 1 件。
 *
 * **寸法は画像のときだけ**（Spec 36）。音声・PDF に寸法は無く、0 で埋めると
 * 「0px の画像」と区別できない — コアの `Attachment` と同じ判断。
 */
export interface PendingAttachment {
  /** 元ファイル名（表示用）。 */
  fileName: string;
  /** 種別（コアの `AttachmentKind` と同じ）。 */
  kind: AttachmentKind;
  /** base64（画像は変換後 WebP、他は無変換のまま）。 */
  dataBase64: string;
  /** 幅（px。画像のときだけ）。 */
  width?: number;
  /** 高さ（px。画像のときだけ）。 */
  height?: number;
  /** 縮小したか。true なら画面にその旨を出す（D3「縮小したことを画面に出す」）。 */
  scaled: boolean;
  /** バイト数。 */
  bytes: number;
}

/**
 * 変換の失敗理由。辞書キー（`chatInput.attach*`）に 1 対 1 で対応する。
 *
 * `readFailed` はファイルそのものを読めなかったとき（#19）。`convertFailed`（変換器が落ちた）と
 * 分けるのは、人の次の手が違うから — 前者はファイルの置き場所や権限を見る、後者はもう一度試す。
 */
export type AttachmentErrorKind =
  | "tooLarge"
  | "convertedTooLarge"
  | "convertFailed"
  | "readFailed"
  | "unsupportedType";

/** 変換の失敗。`kind` で文言を引く（メッセージに生の例外を混ぜない）。 */
export class AttachmentError extends Error {
  /** 失敗の種別。 */
  readonly kind: AttachmentErrorKind;

  constructor(kind: AttachmentErrorKind) {
    super(kind);
    this.kind = kind;
  }
}

/**
 * 長辺が `maxEdge` に収まる寸法を返す（アスペクト比は保つ）。
 *
 * 丸めは長辺が正確に `maxEdge` になる向きで行う — 長辺 = `edge` のとき
 * `edge * (maxEdge / edge) = maxEdge` は誤差なく成立する。
 */
export function fitWithin(
  width: number,
  height: number,
  maxEdge: number = MAX_EDGE_PX,
): { width: number; height: number; scaled: boolean } {
  const edge = Math.max(width, height);
  if (edge <= maxEdge) return { width, height, scaled: false };
  const ratio = maxEdge / edge;
  return {
    width: Math.max(1, Math.round(width * ratio)),
    height: Math.max(1, Math.round(height * ratio)),
    scaled: true,
  };
}

/**
 * バイト列 → base64。
 *
 * `String.fromCharCode(...bytes)` の一括適用は引数上限（数十万）で落ちるので、
 * 32KB ずつ刻む。
 */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/**
 * WebP コンテナのマジックバイト判定（コアの `is_webp` と同じ規則）。
 *
 * worker の `convertToBlob({ type: "image/webp" })` は**希望であって保証ではない** —
 * エンコーダを持たない環境では別形式が返る。確かめずに送ると、コア側の検証で
 * 「WebP ではありません」になり、原因が 1 段遠くなる。
 */
export function isWebpBytes(bytes: Uint8Array): boolean {
  return (
    bytes.length >= 12 &&
    bytes[0] === 0x52 && // R
    bytes[1] === 0x49 && // I
    bytes[2] === 0x46 && // F
    bytes[3] === 0x46 && // F
    bytes[8] === 0x57 && // W
    bytes[9] === 0x45 && // E
    bytes[10] === 0x42 && // B
    bytes[11] === 0x50 // P
  );
}

/** worker への依頼。 */
interface ConvertRequest {
  id: number;
  buffer: ArrayBuffer;
}

/** worker からの返答。 */
export type ConvertResponse =
  | {
      id: number;
      ok: true;
      dataBase64: string;
      width: number;
      height: number;
      scaled: boolean;
      bytes: number;
    }
  | { id: number; ok: false; error: string };

let worker: Worker | null = null;
let requestSeq = 0;
const inFlight = new Map<
  number,
  { resolve: (value: ConvertResponse) => void }
>();

/** worker を（初回だけ）起こす。以後は使い回す。 */
function ensureWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("../workers/imageConvert.ts", import.meta.url), {
    type: "module",
  });
  worker.addEventListener("message", (event: MessageEvent<ConvertResponse>) => {
    const waiter = inFlight.get(event.data.id);
    if (!waiter) return;
    inFlight.delete(event.data.id);
    waiter.resolve(event.data);
  });
  worker.addEventListener("error", () => {
    // worker 自体が死んだら、待っている全員へ失敗を返して作り直せる状態に戻す。
    for (const [id, waiter] of inFlight) {
      waiter.resolve({ id, ok: false, error: "worker crashed" });
    }
    inFlight.clear();
    worker?.terminate();
    worker = null;
  });
  return worker;
}

/**
 * バイト列から種別を判定する（**コアの `detect_format` と同じ規則**）。
 *
 * **ファイル名の拡張子も MIME も見ない。** どちらも書き換えられるので、
 * 信じると中身と種別が食い違ったまま IPC へ流れる。判定が割れたときに
 * 勝つのは**コア側**（保存時に同じ判定をやり直す）で、ここは表示のため。
 */
export function detectKind(bytes: Uint8Array): AttachmentKind | null {
  const at = (i: number, s: string): boolean =>
    s.split("").every((c, k) => bytes[i + k] === c.charCodeAt(0));
  if (isWebpBytes(bytes)) return "image";
  // WAV は WebP と先頭 4 バイトが同じ（どちらも RIFF）。form type まで見る。
  if (bytes.length >= 12 && at(0, "RIFF") && at(8, "WAVE")) return "audio";
  if (bytes.length >= 5 && at(0, "%PDF-")) return "pdf";
  // ftyp は動画専用ではない（HEIC / M4A / QuickTime も名乗る）。ブランドで絞る。
  if (bytes.length >= 12 && at(4, "ftyp")) {
    const brands = ["isom", "iso2", "iso4", "iso5", "iso6", "mp41", "mp42", "avc1", "dash", "mmp4"];
    if (brands.some((b) => at(8, b))) return "video";
    return null;
  }
  if (bytes.length >= 3 && at(0, "ID3")) return "audio";
  // ID3 の無い素の MPEG フレーム同期（先頭 11 bit がすべて 1）。最も緩いので最後。
  if (bytes.length >= 2 && bytes[0] === 0xff && (bytes[1] & 0xe0) === 0xe0) return "audio";
  return null;
}

/**
 * 貼られた生ファイルをどの経路へ流すか（Spec 36）。
 *
 * **既定は画像。** magic で音声・動画・PDF と**確定できたものだけ**が無変換の
 * 経路へ行き、それ以外はすべて画像として変換を試みる。
 *
 * **ここを「画像を magic で当てる」向きに書くと PNG / JPEG / GIF / HEIC が
 * 全部落ちる**（実際に落とした — P4 の退行）。**フロントが見るのは利用者の
 * 生ファイル**で、コアが見るのは**変換後**のバイト列（画像は常に WebP）。
 * [`detectKind`] と Rust の `detect_format` は同じ形の述語に見えるが、
 * **入力の集合が違う** — 層をまたいで述語の形だけを写すと、この差が消える。
 *
 * **画像かどうかを決める権威はブラウザのデコーダ**であって、こちらの表ではない。
 * 表の仕事は「画像でないもの（音声・動画・PDF）を**外す**」ことだけ。
 */
export function routeAttachment(bytes: Uint8Array): AttachmentKind {
  return detectKind(bytes) ?? "image";
}

/**
 * 添付を 1 件受け取り、送信待ちの形にする。
 *
 * **変換するのは画像だけ**（Spec 36 D3）。音声・動画・PDF は無変換で通す —
 * 変換器を同梱すると依存の桁が変わるので、受け付けない形式は入口で断る。
 *
 * **全体を読むのは、大きさと種別で断れないと分かってから**（#19）。順は
 * 「`file.size` をどの種別の上限よりも大きいかで見る → 先頭 [`HEAD_BYTES`] だけ読んで種別を決める →
 * その種別の上限で見る → 全体を読む」。全体を先に読むと、数百 MB の動画を落としたときに
 * その大きさのメモリを確保してから「大きすぎます」になる。
 *
 * @throws {AttachmentError} 上限超・読み込み失敗・デコード不能（= 対応外）・worker の異常。
 */
export async function prepareFile(file: File): Promise<PendingAttachment> {
  if (file.size > largestSourceLimit()) {
    throw new AttachmentError("tooLarge");
  }
  const kind = routeAttachment(new Uint8Array(await readBytes(file.slice(0, HEAD_BYTES))));
  // 画像は元ファイルの上限（変換前）、無変換の種別はその種別の上限をそのまま元ファイルへ掛ける。
  const limit = kind === "image" ? MAX_SOURCE_BYTES : KIND_MAX_BYTES[kind];
  if (file.size > limit) {
    throw new AttachmentError("tooLarge");
  }
  const whole = await readBytes(file);
  if (kind === "image") {
    return convertImageFile(file, whole);
  }
  const bytes = new Uint8Array(whole);
  return {
    fileName: file.name || `attachment.${kind}`,
    kind,
    dataBase64: bytesToBase64(bytes),
    scaled: false,
    bytes: bytes.length,
  };
}

/**
 * 元ファイルとして受け付ける最大の大きさ（#19）。画像の元ファイルの上限と、無変換の種別の上限の
 * 大きい方 — これを超えるファイルは、種別を決める前に断れる。
 *
 * 画像の `KIND_MAX_BYTES.image` は**変換後**の上限なので数えない（元ファイルの門は `MAX_SOURCE_BYTES`）。
 */
function largestSourceLimit(): number {
  return Math.max(MAX_SOURCE_BYTES, KIND_MAX_BYTES.audio, KIND_MAX_BYTES.video, KIND_MAX_BYTES.pdf);
}

/**
 * Blob を読む。失敗は `readFailed`（#19）— 生の例外のまま投げると、呼び手は理由を区別できず
 * 「変換に失敗」と誤った理由を出す。
 */
async function readBytes(blob: Blob): Promise<ArrayBuffer> {
  try {
    return await blob.arrayBuffer();
  } catch {
    throw new AttachmentError("readFailed");
  }
}

/**
 * 画像ファイルを WebP へ変換し、送信待ちの形にする。元ファイルの上限は呼び手が読む前に見ている。
 *
 * @throws {AttachmentError} 変換失敗・変換後 2MB 超。
 */
async function convertImageFile(
  file: File,
  buffer: ArrayBuffer,
): Promise<PendingAttachment> {
  // worker へは transfer で渡す（コピーしない）。transfer した buffer は以後こちらから読めないが、
  // 種別の判定は先頭だけを別に読んで済ませてあるので、ここで使い切ってよい。
  const id = ++requestSeq;
  const response = await new Promise<ConvertResponse>((resolve) => {
    inFlight.set(id, { resolve });
    const request: ConvertRequest = { id, buffer };
    ensureWorker().postMessage(request, [buffer]);
  });
  if (!response.ok) {
    // **デコードできなかった = 対応外の形式**（ここへ来るのは音声・動画・PDF の
    // どれでもないファイル）。worker が死んだ場合だけは別の理由なので分ける —
    // 「形式が悪い」と「変換器が落ちた」で人の次の手が違う（前者は別のファイル、
    // 後者はもう一度試す）。
    throw new AttachmentError(
      response.error === "worker crashed" ? "convertFailed" : "unsupportedType",
    );
  }
  if (response.bytes > MAX_CONVERTED_BYTES) {
    throw new AttachmentError("convertedTooLarge");
  }
  return {
    // 貼り付け（クリップボード）にはファイル名が無いことがある。
    fileName: file.name || "clipboard.png",
    kind: "image",
    dataBase64: response.dataBase64,
    width: response.width,
    height: response.height,
    scaled: response.scaled,
    bytes: response.bytes,
  };
}

/**
 * 保存してある添付の形式から種別を引く（Spec 36）。**形式欄の無い古いレコードは画像**
 * （Spec 23 の時代の添付はすべて WebP の画像だった — コアの既定と対）。生成画像の写し（`jpeg`）も画像。
 */
export function kindOfFormat(format: string | undefined): AttachmentKind {
  switch (format) {
    case "mp3":
    case "wav":
      return "audio";
    case "mp4":
      return "video";
    case "pdf":
      return "pdf";
    default:
      return "image";
  }
}

/**
 * 保存してある添付の形式から MIME を引く（表示の object URL / data URL 用）。
 *
 * **`jpeg` はサーヴァントの返事に付く生成画像の写しだけ**（Spec 76 S76-1）で、利用者が貼る画像は
 * 今も WebP。形式欄の無い古いレコードは WebP（Spec 23 の時代の添付はすべて WebP の画像だった）。
 */
export function attachmentMime(format: string | undefined): string {
  switch (format) {
    case "jpeg":
      return "image/jpeg";
    case "mp3":
      return "audio/mpeg";
    case "wav":
      return "audio/wav";
    case "mp4":
      return "video/mp4";
    case "pdf":
      return "application/pdf";
    default:
      return "image/webp";
  }
}
