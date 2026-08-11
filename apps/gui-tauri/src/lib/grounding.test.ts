import { describe, expect, it } from "vitest";

import {
  groundingView,
  isOpenableUrl,
  sourceIcon,
  sourceLabel,
} from "./grounding";
import type { AgentMessage, Grounding } from "../types";

function message(grounding?: Grounding): AgentMessage {
  return {
    id: "m1",
    from: { kind: "agent", id: "gemmy" },
    to: { kind: "user" },
    content: "調べた結果をお伝えします",
    tokens: 10,
    tsMs: 0,
    hop: 1,
    grounding,
  };
}

describe("groundingView", () => {
  it("接地していない発話は null（大多数の発話に欄を出さない）", () => {
    expect(groundingView(message())).toBeNull();
    expect(groundingView(message({ engine: "google" as const, queries: [], sources: [] }))).toBeNull();
  });

  it("検索語と参照元をそのまま渡す", () => {
    const view = groundingView(
      message({
        engine: "google" as const, queries: ["ザリガニ 生息数"],
        sources: [{ uri: "https://example.test/a", title: "生息数の記事" }],
      }),
    );
    expect(view?.queries).toEqual(["ザリガニ 生息数"]);
    expect(view?.sources).toHaveLength(1);
    expect(view?.sourcesMissing).toBe(false);
  });

  it("検索は起きたが参照元が空なら sourcesMissing を立てる", () => {
    // 「接地していない」ではなく「出典が存在しない」。文言が正反対になるので
    // null へ畳んではいけない。
    const view = groundingView(message({ engine: "google" as const, queries: ["時事"], sources: [] }));
    expect(view).not.toBeNull();
    expect(view?.sourcesMissing).toBe(true);
  });

  it("欄が欠けた旧ログでも落ちない", () => {
    const view = groundingView(
      message({ sources: [{ uri: "https://example.test/a", title: "" }] } as Grounding),
    );
    expect(view?.queries).toEqual([]);
    expect(view?.sources).toHaveLength(1);
  });
});

describe("isOpenableUrl", () => {
  it("http / https だけを通す", () => {
    expect(isOpenableUrl("https://example.test/a")).toBe(true);
    expect(isOpenableUrl("http://example.test/a")).toBe(true);
  });

  it("webview を乗っ取れる形は弾く", () => {
    expect(isOpenableUrl("javascript:alert(1)")).toBe(false);
    expect(isOpenableUrl("file:///C:/Windows/System32")).toBe(false);
    expect(isOpenableUrl("example.test")).toBe(false);
  });
});

describe("sourceLabel", () => {
  it("表題があればそれを出す", () => {
    expect(sourceLabel({ uri: "https://example.test/a", title: "記事" })).toBe("記事");
  });

  it("表題が空ならホスト名で代用する（押せる場所を消さない）", () => {
    expect(sourceLabel({ uri: "https://news.example.test/a", title: "  " })).toBe(
      "news.example.test",
    );
  });

  it("URL として解釈できなければ生の値を出す", () => {
    expect(sourceLabel({ uri: "??", title: "" })).toBe("??");
  });
});

describe("sourceLabel", () => {
  const src = (uri: string, title = "") => ({ uri, title });

  it("表題があればそれを使う", () => {
    expect(sourceLabel(src("https://techcrunch.com/a", "TechCrunch"))).toBe(
      "TechCrunch",
    );
  });

  it("表題が無ければホスト名で代用する", () => {
    expect(sourceLabel(src("https://techcrunch.com/2026/08/08/x"))).toBe(
      "techcrunch.com",
    );
  });

  // X 検索は同じホストの投稿を数十件返す（実機 45 件）。ホスト名で代用すると
  // 並んだ全部が "x.com" になり、リンクの区別が付かない。
  it("X の投稿は投稿者で見分けられる", () => {
    expect(
      sourceLabel(src("https://x.com/hayakawagomi/status/2086373869552845150")),
    ).toBe("@hayakawagomi");
    expect(sourceLabel(src("https://twitter.com/someone/status/12"))).toBe(
      "@someone",
    );
  });

  // x.com/i/status/{id} は X の匿名形式で、i は投稿者ではない。
  // @i と出すと全件が同じ嘘のアカウント名になる（実機で 77 件すべてこの形）。
  it("匿名形式は @i ではなく投稿 ID の末尾で区別する", () => {
    expect(sourceLabel(src("https://x.com/i/status/2017492617358151695"))).toBe(
      "…151695",
    );
    // 別の投稿は別のラベルになる（区別が付くことがこの表示の存在理由）。
    expect(sourceLabel(src("https://x.com/i/status/2017492617358999999"))).toBe(
      "…999999",
    );
  });

  it("status の後ろが数字でなければ投稿として扱わない", () => {
    expect(sourceLabel(src("https://x.com/i/status/foo"))).toBe("x.com");
  });

  // プロフィールや検索結果を投稿として扱うと、押した先が想像と違うものになる。
  it("投稿以外の x.com はホスト名のまま", () => {
    expect(sourceLabel(src("https://x.com/hayakawagomi"))).toBe("x.com");
    expect(sourceLabel(src("https://x.com/search?q=ai"))).toBe("x.com");
  });

  // 空文字を返すとリンクの押せる場所が消える。
  it("URL として読めなくても必ず何かを返す", () => {
    expect(sourceLabel(src("not a url"))).toBe("not a url");
  });
});

describe("groundingView の engine", () => {
  it("記録の engine をそのまま運ぶ", () => {
    for (const engine of ["google", "xai", "open_ai"] as const) {
      const view = groundingView(message({ engine, queries: ["x"], sources: [] }));
      expect(view?.engine).toBe(engine);
    }
  });

  // **OpenAI の出典は普通の web URL** なので、X 専用の分岐は通らない
  // （Spec 34 D6）。アイコンが意味を足すのはラベルがホストを語らないときだけで、
  // ホスト名がそのまま出るならアイコンは同じことを 2 回言う。
  it("open_ai の出典は X 専用の見せ方に落ちない", () => {
    const source = { uri: "https://example.test/article", title: "" };
    expect(sourceLabel(source)).toBe("example.test");
    expect(sourceIcon(source)).toBeNull();
  });

  // engine は Spec 31 で足した欄。それ以前の記録はすべて Google 検索由来で、
  // コア側の serde 既定と同じ向きに揃える。
  it("欄を持たない古い記録は google として読む", () => {
    // redb に保存済みの発話は engine を持たない。型では必須だが、
    // ワイヤから来る実データには欠けうる — その形をそのまま作って読ませる。
    const legacy = { queries: ["x"], sources: [] } as unknown as Grounding;
    expect(groundingView(message(legacy))?.engine).toBe("google");
  });
});

describe("sourceIcon", () => {
  const src = (uri: string, title = "") => ({ uri, title });

  it("X の投稿にだけ付く", () => {
    expect(sourceIcon(src("https://x.com/someone/status/1"))).toBe("x");
    expect(sourceIcon(src("https://twitter.com/someone/status/1"))).toBe("x");
    // 匿名形式にも付く（ラベルが投稿 ID なので、ホストがどこか読めない）。
    expect(sourceIcon(src("https://x.com/i/status/20174926173581"))).toBe("x");
  });

  // ラベルが x.com のままの URL にアイコンを添えると [X] x.com になり、
  // 同じことを 2 回言う。アイコンが意味を足すのは @handle のときだけ。
  it("投稿以外の x.com には付かない", () => {
    expect(sourceIcon(src("https://x.com/someone"))).toBeNull();
    expect(sourceIcon(src("https://x.com/search?q=ai"))).toBeNull();
  });

  it("表題を持つ参照元には付かない（表題が何のページか語っている）", () => {
    expect(sourceIcon(src("https://x.com/a/status/1", "ある投稿"))).toBeNull();
  });

  it("X 以外には付かない", () => {
    expect(sourceIcon(src("https://techcrunch.com/a"))).toBeNull();
    expect(sourceIcon(src("not a url"))).toBeNull();
  });
});
