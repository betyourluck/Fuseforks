import { describe, expect, it, vi } from "vitest";

import { useToolCallDetails } from "./useToolCallDetails";
import type { ToolCallDetail } from "../types";

const detail = (callId: number): ToolCallDetail => ({
  callId,
  args: { pattern: "fn main" },
  argsChars: 21,
  argsTruncated: false,
  body: "src/main.rs:1",
  bodyChars: 13,
  bodyTruncated: false,
});

describe("useToolCallDetails（Spec 57 D7）", () => {
  it("開くと中身を引き、もう一度押すと閉じる。閉じても中身は残る", async () => {
    const fetch = vi.fn(async (id: number) => detail(id));
    const calls = useToolCallDetails(fetch);

    await calls.toggle(7);
    expect(calls.expanded.has(7)).toBe(true);
    expect(calls.details.get(7)).toEqual({ status: "ready", detail: detail(7) });

    await calls.toggle(7);
    expect(calls.expanded.has(7)).toBe(false);
    expect(calls.details.get(7)?.status).toBe("ready");

    // 開き直しても IPC は撃たない。
    await calls.toggle(7);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("引いている最中に閉じて開き直しても、2 本目を撃たない", async () => {
    let resolve: (d: ToolCallDetail) => void = () => {};
    const fetch = vi.fn(
      () => new Promise<ToolCallDetail>((r) => { resolve = r; }),
    );
    const calls = useToolCallDetails(fetch);

    const first = calls.toggle(3); // 開く（引き始める）
    expect(calls.details.get(3)).toEqual({ status: "loading" });
    await calls.toggle(3); // 閉じる
    await calls.toggle(3); // もう一度開く — まだ引いている最中
    expect(fetch).toHaveBeenCalledTimes(1);

    resolve(detail(3));
    await first;
    expect(calls.details.get(3)?.status).toBe("ready");
  });

  it("コアにもう無い（null）はエラーではなく gone。以後は引き直さない", async () => {
    const fetch = vi.fn(async () => null);
    const calls = useToolCallDetails(fetch);

    await calls.toggle(9);
    expect(calls.details.get(9)).toEqual({ status: "gone" });
    await calls.toggle(9);
    await calls.toggle(9);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("IPC が落ちたら failed。押し直せば引き直す", async () => {
    const fetch = vi
      .fn<(id: number) => Promise<ToolCallDetail | null>>()
      .mockRejectedValueOnce(new Error("ipc"))
      .mockResolvedValueOnce(detail(4));
    const calls = useToolCallDetails(fetch);

    await calls.toggle(4);
    expect(calls.details.get(4)).toEqual({ status: "failed" });
    await calls.toggle(4); // 閉じる
    await calls.toggle(4); // 開き直す = 引き直す
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(calls.details.get(4)?.status).toBe("ready");
  });

  it("reset で開閉も中身も捨てる（会話を切り替えたとき）", async () => {
    const calls = useToolCallDetails(async (id) => detail(id));
    await calls.toggle(1);
    calls.reset();
    expect(calls.expanded.size).toBe(0);
    expect(calls.details.size).toBe(0);
  });
});
