/**
 * ファイルのドラッグ&ドロップ（2026-11-16 利用者要望 —「画像ファイルをプロンプトへドラッグして添付にしたい」）。
 *
 * # 受け取り方
 *
 * 窓の `dragDropEnabled` を **false** にして（`tauri.conf.json`）、ページの標準の drop で `File` を受け取る。
 * Tauri の既定（true）は OS からのファイルの drop を横取りし、ページにはパスの一覧だけを渡す — そこから
 * 中身を読むには「画面が渡したパスを読む IPC」が要り、画面から任意のファイルを読める口になる。標準の drop なら
 * 中身は WebView が `File` として渡し、貼り付け・「参照…」と同じ `attach(files)` の経路（変換・上限・断り）に乗る。
 * Spec 23 D4（入口は貼り付けと「参照…」の 2 つ。drop は Tauri が横取りするので入れない）をここで覆した。
 *
 * # ページの外へ落ちた drop
 *
 * 入力欄の外にファイルを落とすと、WebView の既定動作は**そのファイルを開く**（アプリの画面が消えて
 * ファイルが表示される）。`installFileDropGuard` が窓で受け、**ファイルを運ぶドラッグだけ**既定動作を止める。
 * テキストやページ内のドラッグ（エディタの中の文の移動・入力欄へのテキストの drop）には触らない — 窓で
 * `preventDefault` すると、それらの既定動作（文字の挿入）まで消える。入力欄が受けた（`defaultPrevented`）
 * ドラッグにも触らない（入力欄が立てた「コピー」のカーソルを「不可」で上書きしない）。
 */

/** ドラッグがファイルを運んでいるか（OS のエクスプローラーやデスクトップから）。 */
export function carriesFiles(types: Iterable<string> | null | undefined): boolean {
  if (!types) return false;
  for (const type of types) {
    if (type === "Files") return true;
  }
  return false;
}

/** drop されたファイル（無ければ空）。 */
export function droppedFiles(dataTransfer: DataTransfer | null | undefined): File[] {
  return Array.from(dataTransfer?.files ?? []);
}

/** `dragover` / `drop` の最小の形（テストで組み立てられるように）。 */
interface DropEventLike {
  readonly defaultPrevented: boolean;
  readonly dataTransfer: { readonly types: Iterable<string>; dropEffect: string } | null;
  preventDefault(): void;
}

/** 入力欄の外のファイルの drop を止める 1 実装（`dragover` と `drop` の両方で同じ判定）。 */
export function guardFileDrop(event: DropEventLike): void {
  if (event.defaultPrevented || !carriesFiles(event.dataTransfer?.types)) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
}

/**
 * 窓へ張る。戻り値は外す関数（テストのため）。**起動時に 1 回**（`main.ts`）。
 */
export function installFileDropGuard(target: Pick<EventTarget, "addEventListener" | "removeEventListener">): () => void {
  const handler = (event: Event) => guardFileDrop(event as unknown as DropEventLike);
  target.addEventListener("dragover", handler);
  target.addEventListener("drop", handler);
  return () => {
    target.removeEventListener("dragover", handler);
    target.removeEventListener("drop", handler);
  };
}
