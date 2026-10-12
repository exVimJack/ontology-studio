// 工具层测试：useContextUsage.ts 的纯函数
// 覆盖：用量口径（从最新往回找第一条报过 prompt_tokens 的消息）、
// 格式化（128000→"125k"）、缺数据时隐藏。

import { describe, expect, it } from "vitest";
import { computeContextUsage, formatTokens } from "./useContextUsage";
import type { MessageRow } from "@/lib/domain";

function msg(id: string, prompt?: number, completion?: number): MessageRow {
  return {
    id,
    conversation_id: "c",
    role: prompt ? "assistant" : "user",
    status: "complete",
    content: `content-${id}`,
    reasoning: null,
    error: null,
    model: prompt ? "test-model" : null,
    created_at: 0,
    updated_at: 0,
    prompt_tokens: prompt ?? undefined,
    completion_tokens: completion ?? undefined,
    total_tokens:
      prompt != null ? prompt! + (completion ?? 0) : undefined,
  } as MessageRow;
}

describe("formatTokens", () => {
  it("小于 1000 原样显示", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(999)).toBe("999");
  });

  it("十进制格式化：与模型官方口径一致（1M=1000k，非 1024 进制）", () => {
    expect(formatTokens(1000)).toBe("1k");
    expect(formatTokens(1500)).toBe("1.5k");
    expect(formatTokens(59400)).toBe("59.4k");
    expect(formatTokens(100000)).toBe("100k"); // 十进制，不再显示 97.7k 的"缩水"值
    expect(formatTokens(128000)).toBe("128k"); // 模型官方 128K 口径
    expect(formatTokens(1000000)).toBe("1M"); // GLM-5.3 官方 1M
    expect(formatTokens(2000000)).toBe("2M"); // Grok-5
  });
});

describe("computeContextUsage", () => {
  it("无窗口（未配置 provider）→ 全 null", () => {
    const r = computeContextUsage([msg("1", 100, 50)], null);
    expect(r.used).toBeNull();
    expect(r.ratio).toBeNull();
  });

  it("无消息 → used null", () => {
    const r = computeContextUsage([], 128000);
    expect(r.used).toBeNull();
  });

  it("所有消息都没报 usage → 隐藏（used null）", () => {
    const r = computeContextUsage([msg("1"), msg("2")], 128000);
    expect(r.used).toBeNull();
    expect(r.ratio).toBeNull();
  });

  it("取最新一条报过 usage 的消息（prompt+completion）", () => {
    const msgs = [msg("1", 1000, 200), msg("2"), msg("3", 5000, 300)];
    const r = computeContextUsage(msgs, 10000);
    // 最新报 usage 的是 msg3：5000+300=5300，不是 msg1 或总和
    expect(r.used).toBe(5300);
    expect(r.ratio).toBeCloseTo(0.53);
  });

  it("跳过 prompt_tokens=0 的哨兵值（rig 以 0 为未报告）", () => {
    const msgs = [msg("1", 800, 100), msg("2", 0, 0)];
    const r = computeContextUsage(msgs, 10000);
    // msg2 的 0 是未报告哨兵 → 回退到 msg1
    expect(r.used).toBe(900);
  });

  it("超窗口 → ratio 截断为 1（进度条不溢出）", () => {
    const r = computeContextUsage([msg("1", 200000, 50000)], 100000);
    expect(r.ratio).toBe(1);
  });
});
