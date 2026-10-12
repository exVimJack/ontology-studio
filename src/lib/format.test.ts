// 单元测试：format.ts（纯函数格式化器）
// 运行：npm test（vitest）。formatMessageTime 为本次新增，其余为存量函数补齐测试。

import { describe, expect, it } from "vitest"
import {
  compareIngestJobs,
  formatBytes,
  formatCharCount,
  formatMessageTime,
  progressPercent,
} from "./format"

// ── formatMessageTime（新增：会话消息时间戳）──

describe("formatMessageTime", () => {
  const now = new Date(2026, 7, 26, 10, 0) // 2026-08-26 10:00

  it("同日仅显示 HH:mm", () => {
    expect(formatMessageTime(new Date(2026, 7, 26, 9, 5), now)).toBe("09:05")
    expect(formatMessageTime(new Date(2026, 7, 26, 23, 59), now)).toBe("23:59")
    expect(formatMessageTime(new Date(2026, 7, 26, 0, 0), now)).toBe("00:00")
  })

  it("跨日补 MM-DD 前缀", () => {
    expect(formatMessageTime(new Date(2026, 7, 25, 22, 30), now)).toBe("08-25 22:30")
    expect(formatMessageTime(new Date(2025, 11, 31, 8, 0), now)).toBe("12-31 08:00")
  })

  it("分钟与月份补零", () => {
    expect(formatMessageTime(new Date(2026, 7, 26, 7, 3), now)).toBe("07:03")
    expect(formatMessageTime(new Date(2026, 0, 2, 9, 1), now)).toBe("01-02 09:01")
  })

  it("边界：同日 00:00 与 23:59 均属同日", () => {
    expect(formatMessageTime(new Date(2026, 7, 26, 0, 0), now)).toBe("00:00")
    expect(formatMessageTime(new Date(2026, 7, 26, 23, 59), now)).toBe("23:59")
  })
})

// ── formatBytes（存量补测）──

describe("formatBytes", () => {
  it("二进制 1024 进制与单位", () => {
    expect(formatBytes(0)).toBe("0 B")
    expect(formatBytes(1536)).toBe("1.5 KB")
    expect(formatBytes(16_000_000)).toBe("15.3 MB")
  })

  it("null/负数/非有限值返回空串", () => {
    expect(formatBytes(null)).toBe("")
    expect(formatBytes(undefined)).toBe("")
    expect(formatBytes(-1)).toBe("")
    expect(formatBytes(Number.NaN)).toBe("")
  })
})

// ── formatCharCount（存量补测）──

describe("formatCharCount", () => {
  it("小数字直接整数", () => {
    expect(formatCharCount(0)).toBe("0 字符")
    expect(formatCharCount(950)).toBe("950 字符")
  })

  it("大数字紧凑记法", () => {
    expect(formatCharCount(106_640)).toBe("10.7万 字符")
  })

  it("非法输入返回空串", () => {
    expect(formatCharCount(null)).toBe("")
    expect(formatCharCount(-5)).toBe("")
  })
})

// ── progressPercent（存量补测）──

describe("progressPercent", () => {
  it("正常计算并取整", () => {
    expect(progressPercent(50, 134)).toBe(37)
    expect(progressPercent(134, 134)).toBe(100)
    expect(progressPercent(0, 10)).toBe(0)
  })

  it("缺 total / 非法 total 返回 null（非确定态）", () => {
    expect(progressPercent(50, null)).toBeNull()
    expect(progressPercent(null, 134)).toBeNull()
    expect(progressPercent(10, 0)).toBeNull()
    expect(progressPercent(10, -1)).toBeNull()
  })

  it("钳制到 [0,100]（防御估算偏差）", () => {
    expect(progressPercent(150, 100)).toBe(100)
  })
})

// ── compareIngestJobs（存量补测）──

describe("compareIngestJobs", () => {
  it("活动任务优先于完成态", () => {
    const a = { stage: "Parsing", fileName: "b.pdf" }
    const b = { stage: "Done", fileName: "a.pdf" }
    expect(compareIngestJobs(a, b)).toBeLessThan(0)
  })

  it("状态优先级顺序 Parsing < Queued < Error < Cancelled < Done", () => {
    const order = ["Parsing", "Queued", "Error", "Cancelled", "Done"]
    for (let i = 0; i < order.length - 1; i++) {
      expect(
        compareIngestJobs({ stage: order[i], fileName: "x" }, { stage: order[i + 1], fileName: "x" }),
      ).toBeLessThan(0)
    }
  })

  it("同组按文件名自然序（数字感知）", () => {
    expect(
      compareIngestJobs({ stage: "Done", fileName: "a2.pdf" }, { stage: "Done", fileName: "a10.pdf" }),
    ).toBeLessThan(0)
  })

  it("未知 stage 沉底", () => {
    expect(
      compareIngestJobs({ stage: "Done", fileName: "x" }, { stage: "Unknown", fileName: "x" }),
    ).toBeLessThan(0)
  })
})
