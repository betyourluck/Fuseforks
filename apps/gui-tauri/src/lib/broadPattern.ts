/**
 * `run` の許可パターンのうち、**実質的に何でも通る**ものを見分ける（2026-11-11）。
 *
 * 承認画面の「広いほう」は「第 1 引数まで固定して `*`」なので、`python -c <コード>` から
 * `python -c *` が、`pwsh -Command <文>` から `pwsh -Command *` ができる。どちらも、許した後は
 * モデルが渡す文字列をそのまま実行する形で、閉じた許容（Spec 15）の外へ出る。実際の村でも
 * `bash *` と `ssh outcasts *` が許可リストに入っていた。
 *
 * **判定には使わない。** 表示するだけで、コアの `decide` は 1 行も変えない（Spec 15 の
 * 「警告リストは除外リストではない」と同じ線 — 除外リストは載っていないものを通すが、
 * 警告は載っているものに注意を促すだけ）。
 *
 * 一覧は openai/codex が承認から規則を自動で足すときに拒む接頭辞（`core/src/exec_policy.rs` の
 * `BANNED_PREFIX_SUGGESTIONS`）を下敷きにした。あちらは自動の追記を止める仕組みで、こちらは
 * 人が選ぶ前に知らせる仕組み。
 */

/** 広いパターンの種類。辞書の鍵 `broadPattern.{kind}` に対応する。 */
export type BroadKind = "anyCommand" | "anyCode" | "remote";

/** 種類の全部。辞書の鍵と突き合わせる走査テストが読む。 */
export const BROAD_KINDS: readonly BroadKind[] = ["anyCommand", "anyCode", "remote"];

/** 引数の文字列をそのまま実行するもの（シェル）。 */
const SHELLS = new Set([
  "bash", "sh", "zsh", "dash", "ksh", "fish", "csh", "tcsh", "nu",
  "pwsh", "powershell", "cmd", "busybox", "wsl",
]);

/**
 * 後ろに書いたコマンドを起動するもの（ラッパー）。`git` は `-c alias.x=!…` や
 * `core.sshCommand` で任意のコマンドを起動できるので、ここに入れて条件を別に持つ。
 */
const WRAPPERS = new Set([
  "env", "sudo", "doas", "runas", "xargs", "nohup", "nice", "timeout", "start", "watch",
]);

/** コードを引数で受け取って実行するインタプリタと、そのフラグ。 */
const INTERPRETERS: ReadonlyMap<string, readonly string[]> = new Map([
  ["python", ["-c", "-m"]],
  ["python3", ["-c", "-m"]],
  ["py", ["-c", "-m"]],
  ["pythonw", ["-c", "-m"]],
  ["pypy", ["-c", "-m"]],
  ["node", ["-e", "--eval", "-p", "--print"]],
  ["nodejs", ["-e", "--eval", "-p", "--print"]],
  ["deno", ["eval"]],
  ["bun", ["-e", "--eval", "eval"]],
  ["ruby", ["-e"]],
  ["perl", ["-e", "-E"]],
  ["php", ["-r"]],
  ["lua", ["-e"]],
  ["luajit", ["-e"]],
  ["rscript", ["-e"]],
  ["julia", ["-e"]],
  ["osascript", ["-e"]],
]);

/** 遠隔で任意のコマンドを走らせるもの。 */
const REMOTE = new Set(["ssh", "plink", "mosh"]);

/**
 * パターンの先頭のコマンド名を、照合に使う形へ寄せる（パスを落とし、小文字にし、
 * Windows の拡張子を剥がす）。コアの `normalize_command` と同じ向きだが、こちらは
 * 警告だけなので、パス付きの名前（コアでは何にも一致しない）も寄せて知らせる側へ倒す。
 */
function commandName(token: string): string {
  const base = token.split(/[\\/]/).pop() ?? token;
  return base.toLowerCase().replace(/\.(exe|cmd|bat|com)$/, "");
}

/**
 * パターン 1 つの種類を返す。何でも通る形でなければ `null`。
 *
 * 対象は**末尾が `*` のパターンだけ**（コアの照合は完全一致か末尾 `*` の 2 つ）。完全一致は
 * 書いた 1 通りしか通さないので、`bash -c "echo hi"` のような形でも広くない。
 */
export function broadPatternKind(pattern: string): BroadKind | null {
  const tokens = pattern.trim().split(/\s+/).filter((token) => token.length > 0);
  if (tokens.length < 2 || tokens[tokens.length - 1] !== "*") return null;
  const name = commandName(tokens[0]);
  // 先頭のコマンドと末尾の `*` の間（固定した引数）。
  const fixed = tokens.slice(1, -1);

  if (SHELLS.has(name) || WRAPPERS.has(name)) return "anyCommand";
  if (name === "git" && (fixed.length === 0 || fixed.includes("-c"))) return "anyCommand";
  if (REMOTE.has(name)) return "remote";
  const flags = INTERPRETERS.get(name);
  if (flags !== undefined && (fixed.length === 0 || fixed.some((arg) => flags.includes(arg)))) {
    return "anyCode";
  }
  return null;
}

/** 許可リストの中の広いパターン。 */
export interface BroadEntry {
  pattern: string;
  kind: BroadKind;
}

/**
 * `run.json` の本文から、`allow` の広いパターンを並び順のまま拾う。
 *
 * 編集中の本文を読むので、**壊れた JSON は空として扱う**（警告は出さない — 壊れていることは
 * 保存のときにコアが名指しで断る。ここで別の言い方を足さない）。`allow` が配列でない、
 * 要素が文字列でない、も同じく読み飛ばす。
 */
export function broadAllowEntries(runJson: string): BroadEntry[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(runJson);
  } catch {
    return [];
  }
  if (typeof parsed !== "object" || parsed === null) return [];
  const allow = (parsed as { allow?: unknown }).allow;
  if (!Array.isArray(allow)) return [];
  const entries: BroadEntry[] = [];
  for (const pattern of allow) {
    if (typeof pattern !== "string") continue;
    const kind = broadPatternKind(pattern);
    if (kind !== null) entries.push({ pattern, kind });
  }
  return entries;
}
