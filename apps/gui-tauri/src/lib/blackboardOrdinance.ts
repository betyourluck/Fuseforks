/**
 * 条例へ挿入する**黒板の節**と、節があるかの判定（2026-09-17）。
 *
 * 黒板は 2 つに依存している — 条例（利用者が任意に書く文章）と、黒板タブの読み手（固定の綴り）。
 * 新しい村の `Ordinance.md` は空で、コアのプロンプトは黒板に 1 字も触れないので、条例に節が
 * 無い村では付箋が 1 枚も書かれず、黒板タブは理由を言わずに空のまま残る。
 *
 * 処方は 2 つで 1 組 — 条例ダイアログの「黒板の節を挿入」（こちらから渡す）と、黒板タブの
 * 空表示の案内（節が無いことを名指しする）。**挿入した後は利用者の文章**で、アプリは以後
 * 1 字も触らない（条例は利用者の資産）。
 *
 * **綴りの正本はコードの定数**（`BLACKBOARD_DIR` と、`blackboardLanes.ts` の `STATES` / `NOTE_SEPARATOR` / `SUMMARY_NOTE`）。
 * 文面はそこから組むので、状態の集合が変わっても追従する。状態の説明は
 * `Record<BlackboardState, …>` なので、状態を足すと型が説明の欠けを指す。
 *
 * これは現行（Spec 54・条例で運ぶ形）の止血で、Spec 55（書き込みのツール化）が入れば
 * 規約はツールの説明文が運ぶ — そのときこのファイルごと要らなくなる。
 */
import { type BlackboardState, NOTE_SEPARATOR, STATES, SUMMARY_NOTE } from "./blackboardLanes";

/** 作業フォルダの下の黒板のフォルダ名。コアの `blackboard.rs` と同じ綴り。 */
export const BLACKBOARD_DIR = "blackboard";

export type OrdinanceLanguage = "ja" | "en";

const STATE_MEANING: Record<BlackboardState, Record<OrdinanceLanguage, string>> = {
  doing: { ja: "進行中", en: "in progress" },
  "on-hold": {
    ja: "保留。止めるよう言われた仕事",
    en: "on hold; work you were told to pause",
  },
  done: { ja: "完了", en: "finished" },
};

/**
 * 節があるか。見るのは**着手時の置き場の綴り 1 つ**（`blackboard/doing`）。
 *
 * 見出しの文言で探さない — 挿入後は利用者の文章なので、見出しは書き換えられる。
 * 書き換えられても残るのは、サーヴァントに伝えないと黒板が動かない綴りのほう。
 */
export function hasBlackboardSection(ordinance: string): boolean {
  return ordinance.includes(`${BLACKBOARD_DIR}/${STATES[0]}`);
}

/** 挿入する節。末尾は改行 1 つ。 */
export function blackboardSection(language: OrdinanceLanguage): string {
  const dir = `${BLACKBOARD_DIR}/`;
  const first = STATES[0];
  const last = STATES[STATES.length - 1];
  const states = STATES.map((s) => `\`${s}\`（${STATE_MEANING[s].ja}）`).join(" / ");
  const statesEn = STATES.map((s) => `\`${s}\` (${STATE_MEANING[s].en})`).join(" / ");
  const sep = NOTE_SEPARATOR;

  if (language === "en") {
    return [
      "## The village blackboard",
      "",
      `### Notes: \`${dir}<state>/<your display name>${sep}<task name>.md\` — one file per task, grown with append`,
      `- The blackboard is the \`${dir}\` folder in the work folder. It is a working memo shared by the whole village, separate from each servant's long-term memory (Memory).`,
      "- One note per task. Write only to notes that carry your own display name. Never write to someone else's note.",
      `- The state is the folder. There are exactly ${STATES.length}: ${statesEn}. Do not create folders inside a state folder.`,
      `- When you start a new task, create \`${dir}${first}/<your display name>${sep}<task name>.md\` with the \`file\` tool's write (the heading is the task name). After that, add to it with append.`,
      `- In the file name, the first \`${sep}\` (space, hyphen, space) separates the display name from the task name. The task name itself may contain \`${sep}\`.`,
      `- When the state changes, move the file to the other folder with the \`file\` tool's move (for example \`${dir}${first}/Zari${sep}research.md\` → \`${dir}${last}/Zari${sep}research.md\`). Do not pass \`overwrite\`. If the move is refused because the name already exists, report it to the user. Do not create the same name in another folder with write.`,
      '- Do not put characters that file names cannot hold into the task name (`\\ / : * ? " < > |`).',
      "- Write only progress, findings, and hand-over memos. Return the answer to a request as a reply, not as a note.",
      `- Read once, when you start. Right after receiving a new task, list \`${dir}\` with fd and read only your own notes and the ones related to the task. Do not re-read during the same task.`,
      `- Only the coordinator who handed out the work bundles results into \`${dir}${SUMMARY_NOTE}\` with write.`,
      `- Leave finished notes in \`${last}/\`. The user deletes them.`,
      "- For direct instructions from the user, or messages that need an immediate response, skip the blackboard check and respond directly.",
      "",
    ].join("\n");
  }

  return [
    "## 村の黒板",
    "",
    `### 付箋 \`${dir}<状態>/<自分の表示名>${sep}<仕事名>.md\` — 1 仕事 1 ファイル、append で育てる`,
    `- 黒板は作業フォルダの \`${dir}\` フォルダ。村のみんなで共有する作業メモで、各自の長期記憶（Memory）とは別物。`,
    "- 付箋は仕事 1 つに 1 枚。書いてよいのは自分の表示名の付箋だけ。他人の付箋には書かない。",
    `- 状態はフォルダで表す。${STATES.length} つだけ: ${states}。状態フォルダの中にフォルダを作らない。`,
    `- 新しい仕事に着手したら \`${dir}${first}/<自分の表示名>${sep}<仕事名>.md\` を \`file\` の write で作る（見出しは仕事の名前）。以後は append で足す。`,
    `- ファイル名の \`${sep}\`（空白・ハイフン・空白）は最初の 1 つが表示名と仕事名の区切り。仕事名の中に \`${sep}\` があってもよい。`,
    `- 状態が変わったら \`file\` の move でフォルダを移す（例: \`${dir}${first}/ザリ${sep}調査.md\` → \`${dir}${last}/ザリ${sep}調査.md\`）。move に \`overwrite\` を付けない。移せなかったら（同じ名前が既にある）管理人に報告する。write で別のフォルダに同じ名前を作らない。`,
    '- 仕事名にはファイル名に使えない文字を入れない（`\\ / : * ? " < > |`）。',
    "- 書くのは途中経過・気づき・次に渡す人への引き継ぎのメモだけ。頼まれた仕事の答えは付箋ではなく返信で返す。",
    `- 読むのは着手時に 1 回。新しいタスクを受け取った直後に fd で \`${dir}\` を一覧し、自分の付箋と関係する付箋だけ read する。同じタスクの進行中は読み直さない。`,
    `- 依頼を配った進行役だけが \`${dir}${SUMMARY_NOTE}\` を write で束ねる。`,
    `- 終わった仕事の付箋は \`${last}/\` に残す。消すのは管理人。`,
    "- 管理人からの指示や即時対応が要るメッセージでは、黒板の確認を飛ばして直接対応する。",
    "",
  ].join("\n");
}

/**
 * 条例の末尾へ節を足した全文。空の条例にはそのまま入れ、あれば空行 1 つで区切る。
 * 既に節があるなら**何も変えない**（二重に入れない。ボタンの `disabled` と同じ述語）。
 */
export function withBlackboardSection(ordinance: string, language: OrdinanceLanguage): string {
  if (hasBlackboardSection(ordinance)) return ordinance;
  const body = ordinance.replace(/\s+$/, "");
  const section = blackboardSection(language);
  return body === "" ? section : `${body}\n\n${section}`;
}
