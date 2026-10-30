import { describe, expect, it } from "vitest";

import {
  MAX_ATTACHMENTS,
  MAX_MESSAGE_BYTES,
  admit,
  formatMegabytes,
  kindsIn,
  overLimit,
  refKey,
  roomFor,
  splitForSend,
  totalBytes,
  type DraftAttachment,
} from "./attachmentList";
import type { AttachmentKind } from "./carries";

const MB = 1024 * 1024;

function upload(key: string, bytes: number, kind: AttachmentKind = "image"): DraftAttachment {
  return {
    source: "upload",
    key,
    file: { fileName: `${key}.bin`, kind, dataBase64: "", scaled: false, bytes },
  };
}

function ref(id: string, bytes: number, kind: AttachmentKind = "image"): DraftAttachment {
  return { source: "ref", key: refKey(id), ref: { id, fileName: `${id}.webp` }, kind, bytes };
}

describe("上限の値", () => {
  it("コアの ATTACHMENT_MAX_PER_MESSAGE / ATTACHMENT_MAX_MESSAGE_BYTES と同じ", () => {
    expect(MAX_ATTACHMENTS).toBe(5);
    expect(MAX_MESSAGE_BYTES).toBe(12 * MB);
  });
});

describe("admit", () => {
  it("5 件までは足せて、6 件目は件数で断る", () => {
    const four = [upload("a", 1), upload("b", 1), upload("c", 1), upload("d", 1)];
    expect(admit(four, upload("e", 1))).toBeNull();
    expect(admit([...four, upload("e", 1)], upload("f", 1))).toBe("count");
  });

  it("合計が 12 MB を超える 1 件は合計で断り、ちょうどは通す", () => {
    const items = [upload("a", 10 * MB)];
    expect(admit(items, upload("b", 2 * MB))).toBeNull();
    expect(admit(items, upload("b", 2 * MB + 1))).toBe("bytes");
  });

  it("同じ過去の添付は 2 回載せない（件数より先に見る）", () => {
    const items = [ref("x", 1), upload("a", 1), upload("b", 1), upload("c", 1), upload("d", 1)];
    expect(admit(items, ref("x", 1))).toBe("duplicate");
    expect(admit([ref("x", 1)], ref("y", 1))).toBeNull();
  });
});

describe("roomFor", () => {
  it("あと何件載せられるか（負にならない）", () => {
    expect(roomFor([])).toBe(5);
    expect(roomFor([upload("a", 1), ref("x", 1)])).toBe(3);
    const six = ["a", "b", "c", "d", "e", "f"].map((k) => upload(k, 1));
    expect(roomFor(six)).toBe(0);
  });
});

describe("overLimit", () => {
  it("上限の内側では null、件数と合計の超過をそれぞれ名指しする", () => {
    expect(overLimit([upload("a", 12 * MB)])).toBeNull();
    const six = ["a", "b", "c", "d", "e", "f"].map((k) => upload(k, 1));
    expect(overLimit(six)).toBe("count");
    expect(overLimit([upload("a", 12 * MB), ref("x", 1)])).toBe("bytes");
  });
});

describe("splitForSend", () => {
  it("新しく貼ったものは本体ごと、再添付は ID だけに割る", () => {
    const items = [ref("x", 1), upload("a", 3), ref("y", 2, "pdf"), upload("b", 4, "audio")];
    const { uploads, refIds } = splitForSend(items);
    expect(uploads.map((u) => u.fileName)).toEqual(["a.bin", "b.bin"]);
    expect(refIds).toEqual(["x", "y"]);
  });
});

describe("合計と種別", () => {
  it("合計は新規と再添付の両方を数える", () => {
    expect(totalBytes([upload("a", 3), ref("x", 4)])).toBe(7);
  });

  it("種別は重複なしで、貼った順ではなく宣言順（画像・音声・動画・PDF）", () => {
    const items = [upload("a", 1, "pdf"), ref("x", 1, "image"), upload("b", 1, "pdf")];
    expect(kindsIn(items)).toEqual(["image", "pdf"]);
  });

  it("MB 表記は小数 1 桁", () => {
    expect(formatMegabytes(0)).toBe("0.0");
    expect(formatMegabytes(4.2 * MB)).toBe("4.2");
    expect(formatMegabytes(12 * MB)).toBe("12.0");
  });
});
