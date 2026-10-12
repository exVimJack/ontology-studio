// ValidationReport.tsx：覆盖矩阵（线框 07 屏）+ 历史记录（线框 08 屏）+ 验证报告（线框 09 屏）
// 覆盖：29 句话 × 实体映射表，全量 / 仅本次变更切换，缺失项高亮。
// 历史：校验任务存档表（校验# / 时间 / 结果 / 模式 / 对抗轨迹），可展开回放。
// 报告：三栏统计（规则层 / 对抗层 / 覆盖层）+ 对抗轨迹 + issue 明细 + 导出动作。

import { useState } from "react";
import {
  useCoverageMatrix,
  useLatestValidationReview,
  useValidationIssues,
  layerLabel,
} from "@/hooks/useOntologyValidation";
import styles from "./ValidationPanel.module.css";
import own from "./ValidationReport.module.css";

// ── 07 覆盖矩阵（独立视图：全量 / 仅本次变更切换） ──
export function CoverageView({ iri }: { iri: string }) {
  const [mode, setMode] = useState<"full" | "diff">("full");
  const coverage = useCoverageMatrix(iri, mode === "diff" ? ["modified"] : []);

  return (
    <div className={styles.wrap}>
      <div className={styles.toolbar}>
        <span className={styles.toolbarTitle}>29 句话覆盖</span>
        <span className={styles.tagLive}>
          覆盖率 {coverage.data ? `${coverage.data.covered_count} / ${coverage.data.total_count}` : "–"}
        </span>
        <span className={styles.toolbarRight}>
          <button
            type="button"
            className={`${styles.btnGhostSm} ${mode === "full" ? own.btnActive : ""}`}
            onClick={() => setMode("full")}
          >
            全量
          </button>
          <button
            type="button"
            className={`${styles.btnGhostSm} ${mode === "diff" ? own.btnActive : ""}`}
            onClick={() => setMode("diff")}
          >
            仅本次变更 (diff)
          </button>
        </span>
      </div>
      <div className={styles.issueList}>
        {coverage.data && (
          <table className={own.matrix}>
            <thead>
              <tr>
                <th style={{ width: 44 }}>句号</th>
                <th>标准描述</th>
                <th style={{ width: 110 }}>状态</th>
              </tr>
            </thead>
            <tbody>
              {coverage.data.items.map((item) => (
                <tr key={item.sentence_id}>
                  <td>{item.sentence_id}</td>
                  <td className={own.tdDesc}>
                    <div>{item.description}</div>
                    <div className={own.tdShape}>{item.product_shape}</div>
                  </td>
                  <td>
                    {item.covered ? (
                      <span className={`${own.cellStatus} ${own.ok}`}>
                        <span className={own.ic} style={{ background: "#16a34a" }} />
                        已落地
                      </span>
                    ) : (
                      <span className={`${own.cellStatus} ${own.miss}`}>
                        <span className={own.ic} style={{ background: "#dc2626" }} />
                        缺失
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div style={{ display: "flex", gap: 6 }}>
          <button
            type="button"
            className={styles.btnPrimarySm}
            disabled
            title="待语义评审管道接入后启用（缺失项 → A 修订回喂）"
          >
            对缺失项生成修订
          </button>
          <button
            type="button"
            className={styles.btnGhostSm}
            disabled
            title="待接入导出管道"
          >
            导出覆盖报告
          </button>
        </div>
      </div>
    </div>
  );
}

// ── 08 历史记录 ──
export function ValidationHistory({ ontologyIri }: { ontologyIri: string | null }) {
  const issues = useValidationIssues(ontologyIri);
  const coverage = useCoverageMatrix(ontologyIri);
  const review = useLatestValidationReview(ontologyIri);

  const all = issues.data ?? [];
  const total = all.length;
  const disputed = all.filter((i) => String(i.status) === "disputed").length;
  const openErrors = all.filter(
    (i) => i.severity === "error" && ["open", "disputed"].includes(String(i.status)),
  ).length;
  // 是否真的运行过：有 issue 快照或语义评审记录才算
  const ran = total > 0 || review.data != null;
  const resultTag = !ran
    ? { text: "未运行", cls: styles.tagLive }
    : openErrors > 0 || disputed > 0
      ? { text: "未闭环", cls: own.tagDanger }
      : review.data?.status === "in_progress"
        ? { text: "评审进行中", cls: styles.tagLive }
        : { text: "通过", cls: own.tagDone };

  return (
    <div className={styles.wrap}>
      <div className={styles.toolbar}>
        <span className={styles.toolbarTitle}>校验历史</span>
        <span className={own.tagDone}>{ontologyIri ? "本本体" : "全部"}</span>
      </div>
      <div className={styles.issueList} style={{ margin: "0 14px" }}>
        <table className={own.historyTable}>
          <thead>
            <tr>
              <th style={{ width: 80 }}>校验 #</th>
              <th style={{ width: 120 }}>时间</th>
              <th style={{ width: 90 }}>结果</th>
              <th style={{ width: 110 }}>模式</th>
              <th>对抗轨迹</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>#latest</td>
              <td>{new Date().toLocaleString()}</td>
              <td>
                <span className={resultTag.cls}>{resultTag.text}</span>
              </td>
              <td>完整 · 双模型</td>
              <td>
                规则检查 → 覆盖矩阵（{coverage.data?.covered_count ?? "–"}/
                {coverage.data?.total_count ?? "–"}）→ 规则 warning 逐条答辩 → 语义评审 → 人工仲裁 → 出口检查点
              </td>
            </tr>
            {total === 0 && (
              <tr>
                <td colSpan={5} style={{ textAlign: "center", color: "#8c8c8c", padding: 16 }}>
                  暂无校验记录——在「校验任务」阶段①点击「运行校验」后生成存档
                </td>
              </tr>
            )}
          </tbody>
        </table>
        <div className={styles.hint} style={{ textAlign: "left" }}>
          历史存档随 issue 持久化（validation_issues 表）；与 changelog 关联——import 落库对应一条变更记录。
        </div>
      </div>
    </div>
  );
}

// ── 09 验证报告（与校验任务流程一致：① 规则检查 → ② 对抗验证 → ③ 人工仲裁 → ✋ 出口检查点） ──
export function ValidationReport({ ontologyIri }: { ontologyIri: string | null }) {
  const issues = useValidationIssues(ontologyIri);
  const coverage = useCoverageMatrix(ontologyIri);
  const review = useLatestValidationReview(ontologyIri);

  const all = issues.data ?? [];
  const errorCount = all.filter((i) => i.severity === "error").length;
  const warningCount = all.filter((i) => i.severity === "warning").length;
  const sugCount = all.filter((i) => i.severity === "suggestion").length;
  // review 来源 issue：A rejected 的已自动转 disputed（统一发现池 v0.7）
  const rejectedCount = all.filter((i) => i.a_action?.action === "rejected").length;
  const disputedCount = all.filter((i) => String(i.status) === "disputed").length;
  const resolvedCount = all.filter((i) =>
    ["accepted", "overruled"].includes(String(i.status)),
  ).length;
  const openErrors = all.filter(
    (i) => i.severity === "error" && ["open", "disputed"].includes(String(i.status)),
  ).length;

  const ran = all.length > 0 || review.data != null;
  const overall = !ran
    ? { text: "未运行", cls: styles.tagLive }
    : openErrors > 0 || disputedCount > 0
      ? { text: "未闭环", cls: own.tagDanger }
      : review.data?.status === "in_progress"
        ? { text: "评审进行中", cls: styles.tagLive }
        : { text: "通过", cls: own.tagDone };

  // ② 对抗验证状态
  const reviewState = !review.data
    ? { main: "未开始", sub: "启动评审后 B 评审 → A 修订 → B 复核自动轮转" }
    : review.data.status === "in_progress"
      ? { main: `进行中 · 第 ${review.data.round}/${review.data.max_rounds} 轮`, sub: "B 评审 → A 修订 → B 复核" }
      : review.data.status === "resolved"
        ? { main: "已通过", sub: "B 复核裁决 confirm，对抗收敛" }
        : { main: "已转仲裁", sub: "B 复核判 question，分歧升级人工仲裁" };
  const reviewDone = review.data?.status === "resolved";

  // ③ 人工仲裁
  const arbState =
    disputedCount > 0
      ? { main: `${disputedCount} 条待裁决`, sub: "B 判不定的争议在队列中" }
      : { main: resolvedCount > 0 ? "无待裁" : "无", sub: resolvedCount > 0 ? `历史裁决 ${resolvedCount} 条（accepted/overruled）` : "机器已裁决全部，无需人工出场" };

  // ✋ 出口检查点（import 闸门）
  const gateState =
    openErrors > 0 || disputedCount > 0
      ? { main: "阻断", sub: `未闭环 error ×${openErrors} · 未裁决争议 ×${disputedCount}，import 拒绝` }
      : { main: "放行", sub: "无未闭环 error · 无 in_progress 仲裁，import 可通过" };

  return (
    <div className={styles.wrap}>
      <div className={styles.toolbar}>
        <span className={styles.toolbarTitle}>验证报告</span>
        <span className={overall.cls}>{overall.text}</span>
      </div>
      <div className={styles.issueList} style={{ margin: "0 14px" }}>
        <div className={own.report}>
          <div className={own.reportHead}>
            <div>
              <div className={own.reportTitle}>
                对抗校验报告
                <span className={overall.cls} style={{ marginLeft: 6 }}>
                  {overall.text}
                </span>
              </div>
              <div className={own.reportMeta}>
                <span>校验 #latest</span>
                <span>{new Date().toLocaleString()}</span>
                <span>issues {all.length}</span>
              </div>
            </div>
          </div>
          <div className={own.reportStats}>
            <div className={own.rstat}>
              <div className={own.rsTitle}>① 规则检查 · 答辩</div>
              <div className={`${own.rsMain} ${errorCount === 0 ? own.rsOk : own.rsWarn}`}>
                {errorCount === 0 ? "通过" : `${errorCount} error`}
              </div>
              <div className={own.rsSub}>
                检测零 LLM：语法解析 · 结构规范 · 逻辑一致性
                <br />
                error {errorCount} · warning {warningCount} · suggestion {sugCount}
              </div>
            </div>
            <div className={own.rstat}>
              <div className={own.rsTitle}>覆盖矩阵</div>
              <div className={own.rsMain}>
                {coverage.data ? `${coverage.data.covered_count} / ${coverage.data.total_count}` : "–"}
              </div>
              <div className={own.rsSub}>
                正向覆盖{" "}
                {coverage.data ? `${Math.round((coverage.data.coverage ?? 0) * 100)}%` : "–"}
                <br />
                增量 diff：本轮修改实体 {coverage.data?.diff_entities?.length ?? 0}
              </div>
            </div>
            <div className={own.rstat}>
              <div className={own.rsTitle}>② 对抗验证（B 裁决）</div>
              <div className={`${own.rsMain} ${reviewDone ? own.rsOk : own.rsWarn}`}>
                {reviewState.main}
              </div>
              <div className={own.rsSub}>
                {reviewState.sub}
                <br />
                A 拒绝 {rejectedCount} 条 · 已裁决 {resolvedCount} 条
              </div>
            </div>
            <div className={own.rstat}>
              <div className={own.rsTitle}>③ 人工仲裁（殿后）</div>
              <div className={own.rsMain}>{arbState.main}</div>
              <div className={own.rsSub}>{arbState.sub}</div>
            </div>
          </div>
          <div className={own.reportBody}>
            <div className={own.reportSecTitle}>对抗轨迹（可回放）</div>
            <div className={own.reportTl}>
              <div className={own.rtlItem}>
                <span className={`${own.rtlStep} ${own.rtlOk}`}>① 规则检查</span>
                <span className={own.rtlText}>
                  <b>规则引擎：</b>
                  {errorCount + warningCount + sugCount} 项问题（error ×{errorCount} · warning ×
                  {warningCount}）· error 进纠错循环（≤3 轮）
                </span>
              </div>
              <div className={own.rtlItem}>
                <span className={`${own.rtlStep} ${own.rtlB}`}>② 评审</span>
                <span className={own.rtlText}>
                  <b>语义评审：</b>
                  {reviewState.main}——B 八维度评审 → A 修订 → B 复核，自动轮转
                </span>
              </div>
              <div className={own.rtlItem}>
                <span className={`${own.rtlStep} ${own.rtlA}`}>① 答辩</span>
                <span className={own.rtlText}>
                  <b>逐条答辩：</b>
                  A 提论据 + 反证 → 机械匹配 → B 机器裁决（撤销 / 升级 error / 判不定升级仲裁）
                </span>
              </div>
              <div className={own.rtlItem}>
                <span className={`${own.rtlStep} ${own.rtlA}`}>③ 仲裁</span>
                <span className={own.rtlText}>
                  <b>人工仲裁：</b>
                  {arbState.main}——只收轮次封顶与 B 判不定两项
                </span>
              </div>
              <div className={own.rtlItem}>
                <span className={`${own.rtlStep} ${overall.cls === own.tagDone ? own.rtlOk : own.rtlWarn}`}>✋ 检查点</span>
                <span className={own.rtlText}>
                  <b>出口检查点：</b>
                  {gateState.main}——{gateState.sub}
                </span>
              </div>
            </div>
            <div className={own.reportSecTitle}>Issue 明细（{all.length} 条）</div>
            <table className={own.matrix}>
              <thead>
                <tr>
                  <th style={{ width: 60 }}>ID</th>
                  <th style={{ width: 56 }}>层级</th>
                  <th style={{ width: 84 }}>严重级</th>
                  <th>描述</th>
                  <th style={{ width: 100 }}>状态</th>
                </tr>
              </thead>
              <tbody>
                {all.map((issue) => (
                  <tr key={issue.id}>
                    <td style={{ fontFamily: "Fira Code, monospace", fontSize: 11 }}>{issue.id}</td>
                    <td>{layerLabel(issue.layer)}</td>
                    <td>
                      <span
                        className={
                          issue.severity === "error"
                            ? styles.sevError
                            : issue.severity === "warning"
                              ? styles.sevWarning
                              : styles.sevSuggestion
                        }
                      >
                        {issue.severity}
                      </span>
                    </td>
                    <td>{issue.message}</td>
                    <td>
                      <span
                        className={
                          String(issue.status) === "accepted"
                            ? styles.stAccepted
                            : String(issue.status) === "overruled"
                              ? styles.stOverruled
                              : String(issue.status) === "disputed"
                                ? styles.stDisputed
                                : styles.stOpen
                        }
                      >
                        {String(issue.status)}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className={own.reportActions}>
            <button
              type="button"
              className={styles.btnGhostSm}
              onClick={() => {
                const md = [
                  `# 对抗校验报告`,
                  ``,
                  `- 校验 #latest · ${new Date().toLocaleString()}`,
                  `- 总体：${overall.text}`,
                  `- ① 规则检查：error ${errorCount} / warning ${warningCount} / suggestion ${sugCount}`,
                  `- 覆盖矩阵：${coverage.data ? `${coverage.data.covered_count}/${coverage.data.total_count}` : "–"}`,
                  `- ② 对抗验证：${reviewState.main}（A 拒绝 ${rejectedCount} · 已裁决 ${resolvedCount}）`,
                  `- ③ 人工仲裁：${arbState.main}`,
                  ``,
                  `## Issue 明细`,
                  ...all.map(
                    (i) =>
                      `- [${i.severity}] ${layerLabel(i.layer)} ${i.id}: ${i.message}（${i.status}）`,
                  ),
                ].join("\n");
                void navigator.clipboard?.writeText(md);
              }}
            >
              复制 Markdown
            </button>
            <button type="button" className={styles.btnGhostSm}>
              导出 JSON
            </button>
            <button type="button" className={styles.btnPrimarySm}>
              关联变更记录
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
