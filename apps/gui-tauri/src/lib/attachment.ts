/**
 * 添付 — フロント側の判定・変換・上限（Spec 23 = 画像 / Spec 36 = 多モーダル）。
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
import { KIND_MAX_BYTES, type AttachmentKind } from "./carries";

export type { AttachmentKind };

/** 画像の元ファイルの上限（bytes）。デコード前の門。 */
export const MAX_SOURCE_BYTES = 10 * 1024 * 1024;

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

/** 変換の失敗理由。辞書キー（`chatInput.attach*`）に 1 対 1 で対応する。 */
export type AttachmentErrorKind =
  | "tooLarge"
  | "convertedTooLarge"
  | "convertFailed"
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
 * @throws {AttachmentError} 上限超・デコード不能（= 対応外）・worker の異常。
 */
export async function prepareFile(file: File): Promise<PendingAttachment> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const kind = routeAttachment(bytes);
  if (kind === "image") {
    return convertImageFile(file, bytes);
  }
  // 無変換の種別は、その種別の上限をそのまま元ファイルへ掛ける。
  if (bytes.length > KIND_MAX_BYTES[kind]) {
    throw new AttachmentError("tooLarge");
  }
  return {
    fileName: file.name || `attachment.${kind}`,
    kind,
    dataBase64: bytesToBase64(bytes),
    scaled: false,
    bytes: bytes.length,
  };
}

/**
 * 画像ファイルを WebP へ変換し、送信待ちの形にする。
 *
 * @throws {AttachmentError} 10MB 超・変換失敗・変換後 2MB 超。
 */
async function convertImageFile(
  file: File,
  bytes: Uint8Array,
): Promise<PendingAttachment> {
  if (file.size > MAX_SOURCE_BYTES) {
    throw new AttachmentError("tooLarge");
  }
  // worker へは transfer で渡す（コピーしない）。判定で読んだ `bytes` とは
  // 別の buffer を起こす — transfer した側は以後こちらから読めない。
  const buffer = bytes.slice().buffer;
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
