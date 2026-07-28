/**
 * アイコン画像の変換。png / jpg を受け取り、中央の正方形を切り出して
 * 256px 角の WebP に変換する。
 *
 * 変換を UI 層が持つのは、WebView（Chromium）の canvas がデコードとエンコードを
 * 両方持っているため。コア層に画像処理クレートを足すより、既にある機能を使う。
 * コア側は「WebP であること・サイズ上限」だけを検証する（契約は data_contract.yaml）。
 */

/** 出力アイコンの一辺（px）。表示は最大でも 28px 程度なので、Retina を見込んでも十分。 */
const ICON_SIZE = 256;

/** WebP のエンコード品質。アイコン用途では劣化が視認できない範囲で軽くする。 */
const WEBP_QUALITY = 0.85;

/**
 * 画像ファイルを WebP アイコンのバイト列へ変換する。
 *
 * 縦横比が異なる画像は**中央の正方形を切り出す**（cover）。引き伸ばすと顔が歪み、
 * レターボックスにすると丸抜き表示で余白だけが見える。
 */
export async function fileToWebpIcon(file: File): Promise<Uint8Array> {
  const bitmap = await createImageBitmap(file);
  try {
    const side = Math.min(bitmap.width, bitmap.height);
    const sx = (bitmap.width - side) / 2;
    const sy = (bitmap.height - side) / 2;

    const canvas = document.createElement("canvas");
    canvas.width = ICON_SIZE;
    canvas.height = ICON_SIZE;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("canvas 2D コンテキストを取得できません");
    context.drawImage(bitmap, sx, sy, side, side, 0, 0, ICON_SIZE, ICON_SIZE);

    const blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, "image/webp", WEBP_QUALITY),
    );
    if (!blob) throw new Error("WebP への変換に失敗しました");
    return new Uint8Array(await blob.arrayBuffer());
  } finally {
    // ImageBitmap は GC 任せにせず明示的に解放する（デコード済みピクセルを持つ）。
    bitmap.close();
  }
}
