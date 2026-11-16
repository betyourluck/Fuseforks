/**
 * 画像生成の設定ページの配線（Spec 76 P2）。
 *
 * ここが留めているのは**型でも lint でも落ちない 4 つ**:
 *
 * 1. **語の集合が Rust と TS で 2 枚ある**（接続確認の結末・縦横比・解像度・接続先）。ずれると
 *    画面に辞書の鍵名が出るか、選んだ値を Rust が読めずに保存が落ちる
 * 2. **辞書の鍵を実行時に組む**（`settings.imageGen.probe.${語}` ほか）ので、鍵が欠けても
 *    vue-tsc も `usedKeys.test.ts` も通る
 * 3. **開いただけで外へ出ない**（D8）。ページの表示・サーヴァント設定の注記・起動経路から
 *    接続確認を呼ぶ実装へ変わると、押していないのに外へ送る
 * 4. **「いま効いているか」を画面で組み直さない** — Rust の `active` / `inactiveReason` を読む
 */
// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import zh from "../locales/zh-Hans.json";
import {
  IMAGE_ASPECTS,
  IMAGE_RESOLUTIONS,
  PROBE_OUTCOMES,
  PROVIDER_NAMES,
  probeMessageKey,
} from "./imageGenSettings";

const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const coreRs = read("../../../../crates/fuseforks-core/src/image_gen.rs");
const settingsRs = read("../../../../crates/fuseforks-host/src/image_gen_settings.rs");
const page = code(read("../components/ImageGenSettingsPage.vue"));
const agentDialog = code(read("../components/AgentSettingsDialog.vue"));
const settingsDialog = code(read("../components/SettingsDialog.vue"));

/** `impl X { pub fn as_str(...) { match self { Self::A { .. } => "a", ... } } }` の語を読む。 */
function asStrWords(source: string, typeName: string): string[] {
  const start = source.indexOf(`impl ${typeName} {`);
  expect(start, `${typeName} の impl が在ること`).toBeGreaterThan(-1);
  const fnStart = source.indexOf("fn as_str(", start);
  const fnEnd = source.indexOf("\n    }\n", fnStart);
  const body = source.slice(fnStart, fnEnd);
  return [...body.matchAll(/=> "([a-z_]+)"/g)].map((m) => m[1]);
}

/** `pub enum X { A, B }`（serde の snake_case）の語を読む。 */
function enumWords(source: string, typeName: string): string[] {
  const m = new RegExp(`pub enum ${typeName} \\{([\\s\\S]*?)\\n\\}`).exec(source);
  expect(m, `${typeName} が在ること`).not.toBeNull();
  return [...m![1].matchAll(/^\s{4}([A-Z][A-Za-z]*),?$/gm)].map((v) =>
    v[1].replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase(),
  );
}

function lookup(dict: unknown, key: string): unknown {
  return key.split(".").reduce<unknown>(
    (node, part) => (node as Record<string, unknown> | undefined)?.[part],
    dict,
  );
}

describe("語の集合が Rust と一致する", () => {
  it("接続確認の結末 = ok + 失敗の 8 区分 + 送らずに断った 2 つ", () => {
    const failures = asStrWords(coreRs, "ImageGenFailure");
    expect(failures).toHaveLength(8);
    const refused = asStrWords(settingsRs, "InactiveReason").filter((w) => w !== "config_unreadable");
    expect([...PROBE_OUTCOMES].sort()).toEqual(["ok", ...failures, ...refused].sort());
  });

  it("縦横比・解像度・接続先", () => {
    expect([...IMAGE_ASPECTS]).toEqual(enumWords(coreRs, "ImageAspect"));
    expect([...IMAGE_RESOLUTIONS]).toEqual(enumWords(coreRs, "ImageResolution"));
    expect(Object.keys(PROVIDER_NAMES)).toEqual(enumWords(coreRs, "ImageProvider"));
  });
});

describe("実行時に組む辞書の鍵が 3 言語に在る", () => {
  const keys = [
    ...PROBE_OUTCOMES.map(probeMessageKey),
    probeMessageKey("unknown"),
    ...asStrWords(settingsRs, "InactiveReason").map((r) => `settings.imageGen.inactive.${r}`),
    ...IMAGE_ASPECTS.map((a) => `settings.imageGen.aspect.${a}`),
    ...IMAGE_RESOLUTIONS.map((r) => `settings.imageGen.resolution.${r}`),
    "settings.imageGen.probeNeedsProvider",
    "settings.imageGen.probeNeedsKey",
    "settings.imageGen.probeNeedsSave",
  ];
  for (const [name, dict] of [["ja", ja], ["en", en], ["zh-Hans", zh]] as const) {
    it(name, () => {
      for (const key of keys) expect(lookup(dict, key), `${name}: ${key}`).toBeTypeOf("string");
    });
  }

  it("画面は語から鍵を組む（結末の文を直書きしない）", () => {
    expect(page).toContain("$t(probeMessageKey(probe.outcome)");
    expect(page).toContain("$t(`settings.imageGen.inactive.${view.inactiveReason");
  });
});

describe("開いただけで外へ出ない（D8）", () => {
  it("接続確認はボタンの 1 か所からだけ呼ぶ", () => {
    expect(page.match(/ipc\.testImageGen\(/g)).toHaveLength(1);
    const fnStart = page.indexOf("async function testConnection(");
    const fnEnd = page.indexOf("\n}\n", fnStart);
    expect(page.slice(fnStart, fnEnd)).toContain("ipc.testImageGen(");
    expect(page.match(/testConnection\b/g)).toHaveLength(2); // 定義と @click の 2 つだけ
    expect(page).toContain('@click="testConnection"');
  });

  it("開いたときに読むのは設定だけ", () => {
    const mount = page.slice(page.indexOf("onMounted("), page.indexOf("});", page.indexOf("onMounted(")));
    expect(mount).toContain("ipc.getImageGenSettings()");
    expect(mount).not.toContain("testImageGen");
  });

  it("サーヴァント設定の注記も読むだけ", () => {
    expect(agentDialog).toContain("ipc.getImageGenSettings()");
    expect(agentDialog).not.toContain("testImageGen");
  });

  it("Rust の起動経路は接続確認を呼ばない", () => {
    const boot = code(read("../../../../crates/fuseforks-host/src/boot.rs"));
    const commands = code(read("../../src-tauri/src/commands.rs"));
    expect(boot).not.toContain("image_gen_settings::probe(");
    expect(commands.match(/image_gen_settings::probe\(/g)).toHaveLength(1);
    const fnStart = commands.indexOf("pub async fn test_image_gen(");
    expect(fnStart).toBeGreaterThan(-1);
    expect(commands.indexOf("image_gen_settings::probe(")).toBeGreaterThan(fnStart);
  });
});

describe("変えた直後に差し込み口を埋め直す", () => {
  it("保存・鍵の登録・鍵の削除の 3 本とも apply_and_note を呼ぶ（呼ばないと再起動まで効かない）", () => {
    const commands = code(read("../../src-tauri/src/commands.rs"));
    for (const name of ["set_image_gen_settings", "set_image_gen_key", "clear_image_gen_key"]) {
      const start = commands.indexOf(`pub async fn ${name}(`);
      expect(start, name).toBeGreaterThan(-1);
      const end = commands.indexOf("\n}\n", start);
      expect(commands.slice(start, end), name).toContain("image_gen_settings::apply_and_note(");
    }
  });
});

describe("判定は Rust の 1 実装を読む", () => {
  it("いま効いているかは view.active / inactiveReason から", () => {
    expect(page).toContain('view.active ? \'text-run\' : \'text-ink-dim\'');
    expect(agentDialog).toContain("(await ipc.getImageGenSettings()).active");
    // 置き場（image の行の直下）は agentToolNotesWiring.test.ts が留める。
    expect(agentDialog).toContain("isToolChecked('image') && imageGenActive === false");
  });

  it("Meta の間は解像度を押せない（値は保存したまま）", () => {
    expect(page).toMatch(
      /:disabled="busy \|\| blocked \|\| !view \|\| row\?\.honorsResolution === false"\s+data-image-gen-resolution/,
    );
  });
});

describe("目録の置き場（外部連携の Jev の下）", () => {
  it("Jev → 画像生成 → 単価表 の順", () => {
    const jev = settingsDialog.indexOf('"settings.menuJev"');
    const imageGen = settingsDialog.indexOf('"settings.menuImageGen"');
    const pricing = settingsDialog.indexOf('"settings.menuPricing"');
    expect(jev).toBeGreaterThan(-1);
    expect(imageGen).toBeGreaterThan(jev);
    expect(pricing).toBeGreaterThan(imageGen);
    expect(settingsDialog).toContain('<ImageGenSettingsPage v-else-if="page === \'imageGen\'" />');
  });
});
