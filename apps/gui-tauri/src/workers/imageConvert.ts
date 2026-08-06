/// <reference lib="webworker" />
/**
 * 画像 → WebP 変換の WebWorker（Spec 23 P4）。
 *
 * メインスレッドでやらない理由はデコードとエンコードが CPU バウンドだから —
 * 数 MB の画像で描画が止まる（rev2 査読 6）。判断はしない: 寸法の規則も
 * WebP の判定も `lib/attachment.ts` の純関数を使い、ここは器だけを持つ。
 */
import {
  MAX_EDGE_PX,
  WEBP_QUALITY,
  bytesToBase64,
  fitWithin,
  isWebpBytes,
  type ConvertResponse,
} from "../lib/attachment";

self.addEventListener(
  "message",
  (event: MessageEvent<{ id: number; buffer: ArrayBuffer }>) => {
    void convert(event.data.id, event.data.buffer);
  },
);

async function convert(id: number, buffer: ArrayBuffer): Promise<void> {
  try {
    // 形式の判定はデコーダに任せる（png / jpg / webp / gif …）。
    // 壊れたバイト列や非対応形式はここで例外になる。
    const bitmap = await createImageBitmap(new Blob([buffer]));
    const { width, height, scaled } = fitWithin(
      bitmap.width,
      bitmap.height,
      MAX_EDGE_PX,
    );
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2d context unavailable");
    ctx.drawImage(bitmap, 0, 0, width, height);
    bitmap.close();

    const blob = await canvas.convertToBlob({
      type: "image/webp",
      quality: WEBP_QUALITY,
    });
    const bytes = new Uint8Array(await blob.arrayBuffer());
    // `type` は希望であって保証ではない。エンコーダを持たない環境では
    // 別形式が返るので、ここで確かめて正直に失敗する（コアの検証まで
    // 運んでから落とすと原因が 1 段遠くなる）。
    if (!isWebpBytes(bytes)) {
      throw new Error("webp encoder unavailable");
    }

    const response: ConvertResponse = {
      id,
      ok: true,
      dataBase64: bytesToBase64(bytes),
      width,
      height,
      scaled,
      bytes: bytes.length,
    };
    self.postMessage(response);
  } catch (error) {
    const response: ConvertResponse = { id, ok: false, error: String(error) };
    self.postMessage(response);
  }
}
