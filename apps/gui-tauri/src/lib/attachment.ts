/**
 * 画像の添付 — フロント側の変換と上限（Spec 23 P4）。
 *
 * 「フロントは常に WebP を作る」（D3）の実装。重い処理（デコード・縮小・
 * エンコード）は WebWorker（`workers/imageConvert.ts`）で行い、
 * メインスレッドを止めない。このファイルは**純関数と worker の窓口**だけを持ち、
 * 純関数は vitest で固定する。
 *
 * 上限は 2 段（D5）:
 * - 元ファイル 10MB — **デコードの前**に見る。20MB の HEIC をデコードして
 *   から断ると、断るまでの間メモリと時間を払う
 * - 変換後 2MB — コア側（`validate_attachment`）と同じ値。フロントで先に
 *   見るのは、IPC を往復してから拒否されるより手前で分かるほうが速いだけで、
 *   **門はコア側が本体**（IPC を直接叩く経路はコアが塞ぐ）
 */

/** 元ファイルの上限（bytes）。デコード前の門。 */
export const MAX_SOURCE_BYTES = 10 * 1024 * 1024;

/** 変換後（WebP）の上限（bytes）。コアの `ATTACHMENT_MAX_BYTES` と同じ値。 */
export const MAX_CONVERTED_BYTES = 2 * 1024 * 1024;

/** 長辺の上限（px）。コアの `ATTACHMENT_MAX_EDGE_PX` と同じ値。 */
export const MAX_EDGE_PX = 1568;

/** WebP の品質（lossy）。スクリーンショットの文字が読める範囲で小さく。 */
export const WEBP_QUALITY = 0.85;

/** 送信を待っている添付 1 枚。 */
export interface PendingAttachment {
  /** 元ファイル名（表示用）。 */
  fileName: string;
  /** 変換後 WebP の base64。 */
  dataBase64: string;
  /** 変換後の幅（px）。 */
  width: number;
  /** 変換後の高さ（px）。 */
  height: number;
  /** 縮小したか。true なら画面にその旨を出す（D3「縮小したことを画面に出す」）。 */
  scaled: boolean;
  /** 変換後のバイト数。 */
  bytes: number;
}

/** 変換の失敗理由。辞書キー（`chatInput.attach*`）に 1 対 1 で対応する。 */
export type AttachmentErrorKind =
  | "tooLarge"
  | "convertedTooLarge"
  | "convertFailed";

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
 * 画像ファイルを WebP へ変換し、送信待ちの形にする。
 *
 * @throws {AttachmentError} 10MB 超・変換失敗・変換後 2MB 超。
 */
export async function convertImageFile(file: File): Promise<PendingAttachment> {
  if (file.size > MAX_SOURCE_BYTES) {
    throw new AttachmentError("tooLarge");
  }
  const buffer = await file.arrayBuffer();
  const id = ++requestSeq;
  const response = await new Promise<ConvertResponse>((resolve) => {
    inFlight.set(id, { resolve });
    const request: ConvertRequest = { id, buffer };
    // buffer は transfer で渡す（コピーしない）。以後こちら側からは読めない。
    ensureWorker().postMessage(request, [buffer]);
  });
  if (!response.ok) {
    throw new AttachmentError("convertFailed");
  }
  if (response.bytes > MAX_CONVERTED_BYTES) {
    throw new AttachmentError("convertedTooLarge");
  }
  return {
    // 貼り付け（クリップボード）にはファイル名が無いことがある。
    fileName: file.name || "clipboard.png",
    dataBase64: response.dataBase64,
    width: response.width,
    height: response.height,
    scaled: response.scaled,
    bytes: response.bytes,
  };
}
