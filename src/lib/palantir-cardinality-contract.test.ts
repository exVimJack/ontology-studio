/**
 * 契约测试：前端基数枚举必须与 Rust 后端真相源一致。
 *
 * 背景（历史 bug）：前端曾自造 Palantir 四元组（ONE_TO_ONE/...），后端
 * store.rs::validate_link 只接受 Gaia 的 ONE|MANY，导致「新增关系」无论选哪个
 * 基数都保存失败，而当时的前端测试按四元组断言，把错误固化成绿色。
 *
 * 本测试直接读 Rust 源码（vite `?raw`）提取允许值，与 CARDINALITIES 对撞
 * ——后端改了这里就红。
 */
import { describe, expect, it } from "vitest";
import storeRs from "../../crates/ontology-store/src/store.rs?raw";
import { CARDINALITIES } from "./palantir-modeling";

function rustAllowedCardinalities(): string[] {
    // 匹配：if l.cardinality != "ONE" && l.cardinality != "MANY" { ... }
    const line = storeRs
        .split("\n")
        .find((l) => l.includes("l.cardinality !="));
    if (!line) throw new Error("store.rs 里找不到 cardinality 校验行，契约可能已改");
    return [...line.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

describe("cardinality 跨端契约（前端 CARDINALITIES ↔ 后端 validate_link）", () => {
    it("提取到后端允许值（防契约漂移时静默通过）", () => {
        expect(rustAllowedCardinalities().length).toBeGreaterThan(0);
    });

    it("前端枚举与后端完全一致（顺序无关）", () => {
        expect([...CARDINALITIES].sort()).toEqual(
            rustAllowedCardinalities().sort(),
        );
    });
});
