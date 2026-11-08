/**
 * 発話の中のファイルの位置（2026-11-08）。
 *
 * 起点は利用者 —「メッセージの中にあるファイルの位置をファイラー（エクスプローラー）やエディタで
 * 開けるようにしたい」。**押せるようにするのはバッククォートの中のパスと、パスの形をした Markdown
 * リンクの行き先だけ**（地の文は拾わない — 誤検出が多く、モデルはパスをたいていバッククォートで書く）。
 *
 * ここは文字列の形だけを見る純関数。**実在するか・どこを指すかはコアが決める**（相対パスは発言した
 * 個体の作業フォルダから解決する。IPC `resolve_message_path`）。形の判定を緩めすぎると、押しても
 * 「見つかりません」になるだけの下線が増えるので、区切りの無い名前（`world.json`）は拡張子の閉じた
 * 許容で受ける — `state.typing` のようなコードの識別子を拾わないため。
 */

/** 発話から読み取ったパスと、`path:42` の形なら行番号。 */
export interface PathRef {
  path: string;
  line: number | null;
}

/**
 * 区切りの無い名前をパスとして受ける拡張子（小文字）。**区切りを含むパスには掛けない**
 * （`src/foo.weird` は区切りがあるので受ける）。足すときはここに 1 語足す。
 */
const BARE_EXTENSIONS = new Set([
  "md", "txt", "log", "json", "jsonl", "toml", "yaml", "yml", "ini", "cfg", "conf", "env", "csv", "tsv",
  "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte", "css", "scss", "html", "htm", "xml", "svg",
  "py", "rb", "go", "java", "kt", "swift", "c", "h", "cpp", "hpp", "cs", "php", "lua", "sh", "ps1", "bat",
  "sql", "lock", "pdf", "png", "jpg", "jpeg", "webp", "gif", "mp4", "mp3", "wav", "zip", "redb", "tla",
]);

/** パスに現れない（現れたらコードや式と読む）文字。 */
const NOT_IN_PATH = /[<>"|*?(){}`;,=]/;

/**
 * 文字列がファイルの位置に見えるなら `PathRef`、見えなければ `null`。
 *
 * - URL（`scheme://`）は除く — それは `lib/links.ts` の側
 * - `path:42` / `path:42:7` は行番号を取り出す（ドライブの `C:` は直後が区切りなので当たらない）
 * - 区切り（`/` か `\`）を含むか、ドットファイル（`.gitignore`）か、`名前.拡張子` で拡張子が許容に入るもの
 * - 空白は絶対パス（`C:\` / `/`）のときだけ許す（`Program Files`）
 */
export function parsePathRef(text: string): PathRef | null {
  const raw = text.trim();
  if (!raw || raw.length > 400) return null;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(raw)) return null;
  if (NOT_IN_PATH.test(raw) || raw.includes("::")) return null;

  let path = raw;
  let line: number | null = null;
  const suffix = /:(\d+)(?::\d+)?$/.exec(raw);
  if (suffix) {
    path = raw.slice(0, suffix.index);
    line = Number(suffix[1]);
  }
  if (!path || path.startsWith("-")) return null;

  const absolute = /^[a-z]:[\\/]/i.test(path) || path.startsWith("/") || path.startsWith("\\\\");
  if (/\s/.test(path) && !absolute) return null;
  if (!/[a-z]/i.test(path)) return null;

  const separated = /[\\/]/.test(path);
  if (separated) return { path, line };
  if (/^\.[\w-]+$/.test(path)) return { path, line };
  const bare = /^[\w.-]+\.([a-z][a-z0-9]{0,9})$/i.exec(path);
  if (bare && BARE_EXTENSIONS.has(bare[1].toLowerCase())) return { path, line };
  return null;
}
