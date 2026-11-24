// UI 层：OntologyVersionsPanel.tsx（本体版本管理面板）
//
// 显式发布才留快照；回退切指针不新建版本。展示：
//   ① 状态行：当前内容已发布（版本标签）/ 未发布（需发布建立基线或重新发布）
//   ② 发布按钮（表单：label + description，留空自动生成 v0.0.N）
//   ③ 版本列表：版本标签/发布序号/说明/triple_count/时间/发布者；active 标"已启用"；
//      每行自带"修改点"（发布时落库，可展开完整列表）；"启用此版本"在行头最右侧
//   ④ 版本上限设置（默认 20，FIFO 清理，启用版本不删）

import { useEffect, useState } from "react";
import { CheckCircle2, GitBranch, Plus, RefreshCw, Trash2 } from "lucide-react";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  useDeleteOntologyVersion,
  useOntologyActiveRevision,
  useOntologyVersions,
  useOntologyWorkspaceDirty,
  useRevertOntologyVersion,
  useSetVersionLimit,
  useVersionLimit,
} from "@/hooks/useOntologyTtl";
import type { TtlVersionDiff, TtlVersionSummary } from "@/lib/domain";
import {
  groupDiffListItems,
  projectDiffListItems,
  type DiffListOp,
  type DiffRevokeTarget,
} from "@/lib/ontology-diff-display";
import { PublishOntologyVersionDialog } from "@/components/ontology/PublishOntologyVersionDialog";
import styles from "./OntologyVersionsPanel.module.css";

export function OntologyVersionsPanel({ ontologyIri }: { ontologyIri: string | null }) {
  const versionsQ = useOntologyVersions(ontologyIri);
  const activeQ = useOntologyActiveRevision(ontologyIri);
  const dirtyQ = useOntologyWorkspaceDirty(ontologyIri);
  const limitQ = useVersionLimit();
  const revert = useRevertOntologyVersion();
  const delVersion = useDeleteOntologyVersion();
  const setLimit = useSetVersionLimit();

  const versions = versionsQ.data ?? [];
  const activeRev = activeQ.data ?? null;
  const dirty = dirtyQ.data ?? false;
  const published = activeRev != null;

  const [showPublish, setShowPublish] = useState(false);
  const [limitInput, setLimitInput] = useState<string>("");
  // 当前生效版本的标签（状态行显示用）
  const activeLabel = activeRev != null
    ? (versions.find((v) => v.revision === activeRev)?.label ?? `第 ${activeRev} 次发布`)
    : null;

  useEffect(() => {
    if (limitQ.data != null) setLimitInput(String(limitQ.data));
  }, [limitQ.data]);

  /**
   * 启用历史版本：当前内容将被该快照覆盖，属破坏性操作 —— 必须 await 到确认结果。
   * 不用 window.confirm：它被 tauri-plugin-dialog 注入脚本覆盖成返回 Promise 的异步函数，
   * `!window.confirm(x)` 恒为 false，用户点取消也会写入。
   */
  const doRevert = async (v: TtlVersionSummary) => {
    if (!ontologyIri) return;
    const ok = await confirm(
      `启用「${v.label || `v0.0.${v.revision}`}」（第 ${v.revision} 次发布）？\n当前内容将被该版本快照覆盖（未发布的修改会丢失）。\n回退可逆：所有已发布版本仍在，随时可再切回去。`,
      { kind: "warning" },
    );
    if (!ok || revert.isPending) return;
    revert.mutate({ iri: ontologyIri, revision: v.revision });
  };

  /** 删除版本快照：不可恢复，同样必须 await 确认。 */
  const doDelete = async (v: TtlVersionSummary) => {
    if (!ontologyIri) return;
    const ok = await confirm(
      `删除版本「${v.label || `v0.0.${v.revision}`}」（第 ${v.revision} 次发布）？\n该版本快照将被永久删除，无法恢复。`,
      { kind: "warning" },
    );
    if (!ok || delVersion.isPending) return;
    delVersion.mutate({ iri: ontologyIri, revision: v.revision });
  };

  const doSetLimit = () => {
    const n = Number.parseInt(limitInput, 10);
    if (Number.isNaN(n) || n <= 0) return;
    setLimit.mutate(n);
  };

  return (
    <div className={styles.panel}>
      {/* 状态行：从未发布 / 已发布 / 已发布且有未发布修改 */}
      <div className={styles.statusRow}>
        {!published ? (
          <>
            <GitBranch size={14} />
            <span>尚未发布——导出与对外使用需先发布版本</span>
          </>
        ) : dirty ? (
          <>
            <GitBranch size={14} />
            <span>
              当前生效：<b>{activeLabel ?? `第 ${activeRev} 次发布`}</b>
              ；存在未发布修改（导出仍为生效版）
            </span>
          </>
        ) : (
          <>
            <CheckCircle2 size={14} />
            <span>
              当前已发布：<b>{activeLabel ?? `第 ${activeRev} 次发布`}</b>
            </span>
          </>
        )}
        <button className={styles.publishBtn} onClick={() => setShowPublish(true)}>
          <Plus size={13} /> 发布版本
        </button>
      </div>

      {ontologyIri && (
        <PublishOntologyVersionDialog
          ontologyIri={ontologyIri}
          open={showPublish}
          onClose={() => setShowPublish(false)}
        />
      )}

      {/* 版本列表 */}
      <div className={styles.sectionTitle}>版本历史</div>
      {versionsQ.isLoading ? (
        <div className={styles.empty}>加载中…</div>
      ) : versions.length === 0 ? (
        <div className={styles.empty}>
          还没有已发布版本。版本 = 显式发布的里程碑（import/编辑的中间过程只记变更日志），
          发布后可对比、回退。
        </div>
      ) : (
        <div className={styles.list}>
          {versions.map((v) => {
            const isActive = v.revision === activeRev;
            return (
              <div key={v.revision} className={`${styles.item} ${isActive ? styles.itemActive : ""}`}>
                <div className={styles.itemHead}>
                  <span className={styles.rev}>{v.label || `v0.0.${v.revision}`}</span>
                  {isActive && <span className={styles.badge}>已启用</span>}
                  <span className={styles.meta}>
                    第 {v.revision} 次发布 · {v.triple_count} triples ·{" "}
                    {new Date(v.created_at).toLocaleString()} · {v.author}
                  </span>
                  {!isActive && (
                    <button
                      className={styles.revertBtn}
                      onClick={() => doRevert(v)}
                      disabled={revert.isPending}
                    >
                      <RefreshCw size={12} /> 启用
                    </button>
                  )}
                  <button
                    className={styles.deleteBtn}
                    onClick={() => doDelete(v)}
                    disabled={isActive || delVersion.isPending}
                    title={isActive ? "当前启用版本不能删除，请先启用其他版本" : "删除该版本快照"}
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
                {v.description && <div className={styles.desc}>{v.description}</div>}
                <VersionDiffInline diffSummary={v.diff_summary} />
              </div>
            );
          })}
        </div>
      )}

      {/* 版本上限 */}
      <div className={styles.sectionTitle}>版本上限（FIFO 清理，启用中的版本不删）</div>
      <div className={styles.limitRow}>
        <input
          className={styles.inputSmall}
          type="number"
          min={1}
          value={limitInput}
          onChange={(e) => setLimitInput(e.target.value)}
        />
        <button className={styles.ghostBtn} onClick={doSetLimit} disabled={setLimit.isPending}>
          保存
        </button>
        <span className={styles.hint}>超限时自动删除最旧版本</span>
      </div>
    </div>
  );
}

/** 单版本行内的修改点（发布时落库的 diff_summary）：摘要 + 可展开完整列表。 */
function VersionDiffInline({ diffSummary }: { diffSummary?: string }) {
  if (!diffSummary) return null; // 老数据无修改点字段
  let diff: TtlVersionDiff | null = null;
  try {
    const parsed = JSON.parse(diffSummary);
    // 必须校验结构：空对象 {}（首版）或畸形数据直接跳过，防 undefined.length 崩溃
    if (
      parsed &&
      typeof parsed === "object" &&
      Array.isArray(parsed.added_classes) &&
      Array.isArray(parsed.removed_classes) &&
      Array.isArray(parsed.modified_classes) &&
      Array.isArray(parsed.added_properties) &&
      Array.isArray(parsed.removed_properties) &&
      Array.isArray(parsed.modified_properties) &&
      typeof parsed.ontology_meta_changed === "boolean"
    ) {
      diff = {
        ...parsed,
        added_subclass_of: parsed.added_subclass_of ?? [],
        removed_subclass_of: parsed.removed_subclass_of ?? [],
      };
    }
  } catch {
    diff = null;
  }
  if (!diff) return null; // 首版或异常数据不显示
  const counts = {
    addedClasses: diff.added_classes.length,
    removedClasses: diff.removed_classes.length,
    modifiedClasses: diff.modified_classes.length,
    addedProps: diff.added_properties.length,
    removedProps: diff.removed_properties.length,
    modifiedProps: diff.modified_properties.length,
    addedHier: diff.added_subclass_of?.length ?? 0,
    removedHier: diff.removed_subclass_of?.length ?? 0,
    meta: diff.ontology_meta_changed,
  };
  const total =
    counts.addedClasses +
    counts.removedClasses +
    counts.modifiedClasses +
    counts.addedProps +
    counts.removedProps +
    counts.modifiedProps +
    counts.addedHier +
    counts.removedHier +
    (counts.meta ? 1 : 0);
  if (total === 0) {
    // 内容与上一版本相同（无修改点）——仍给出明确提示，不让用户以为记录丢了
    return (
      <div className={styles.diffInlineEmpty}>
        <span className={styles.diffInlineTag}>修改点</span>
        本次发布与上一版本无差异
      </div>
    );
  }
  const summaryBits: string[] = [];
  if (counts.addedClasses) summaryBits.push(`+${counts.addedClasses} 类`);
  if (counts.removedClasses) summaryBits.push(`−${counts.removedClasses} 类`);
  if (counts.modifiedClasses) summaryBits.push(`~${counts.modifiedClasses} 类`);
  if (counts.addedProps) summaryBits.push(`+${counts.addedProps} 属性`);
  if (counts.removedProps) summaryBits.push(`−${counts.removedProps} 属性`);
  if (counts.modifiedProps) summaryBits.push(`~${counts.modifiedProps} 属性`);
  if (counts.addedHier) summaryBits.push(`+${counts.addedHier} 继承`);
  if (counts.removedHier) summaryBits.push(`−${counts.removedHier} 继承`);
  if (counts.meta) summaryBits.push("~本体头");
  return (
    <details className={styles.diffInline}>
      <summary className={styles.diffInlineSummary}>
        <span className={styles.diffInlineTag}>修改点</span>
        {summaryBits.join(" · ")}
      </summary>
      <div className={styles.diffInlineBody}>
        <DiffView diff={diff} showTitle={false} />
      </div>
    </details>
  );
}

const OP_GLYPH: Record<DiffListOp, string> = {
  add: "+",
  remove: "−",
  modify: "~",
};

function DiffView({
  diff,
  loading,
  showTitle = true,
  labelMap,
  onRevoke,
  revokePending,
  getRevokeBlockReason,
}: {
  diff: TtlVersionDiff | null;
  loading?: boolean;
  showTitle?: boolean;
  /** 类 IRI → 中文名（属性项优先用自身 label） */
  labelMap?: Map<string, string> | null;
  /** 提供时显示行级悬停「撤销」 */
  onRevoke?: (target: DiffRevokeTarget) => void;
  revokePending?: boolean;
  /** 返回非空则禁用该行撤销并作 title 提示（如删除态禁止序） */
  getRevokeBlockReason?: (target: DiffRevokeTarget) => string | null;
}) {
  if (loading) return <div className={styles.empty}>对比中…</div>;
  if (!diff) return <div className={styles.empty}>选择两个版本开始对比。</div>;
  const groups = groupDiffListItems(projectDiffListItems(diff, labelMap));
  const vsWorkspace = diff.rev_b === 0;
  if (groups.length === 0) {
    return (
      <div className={styles.diffEmpty}>
        {vsWorkspace
          ? "工作区与生效版无差异"
          : `rev ${diff.rev_a} vs rev ${diff.rev_b}：无差异`}
      </div>
    );
  }

  return (
    <div className={styles.diff}>
      {showTitle && (
        <div className={styles.diffTitle}>
          {vsWorkspace
            ? "工作区相对生效版的修改点"
            : `rev ${diff.rev_a} → rev ${diff.rev_b} 的修改点`}
        </div>
      )}
      {groups.map((g) => (
        <div key={g.section} className={styles.diffSection}>
          <div className={styles.diffSectionTitle}>{g.label}</div>
          {g.items.map((it) => {
            const tone =
              it.op === "add"
                ? styles.add
                : it.op === "remove"
                  ? styles.del
                  : styles.mod;
            const blockReason = getRevokeBlockReason?.(it.revoke) ?? null;
            return (
              <div
                key={it.key}
                className={`${styles.diffLine} ${styles.diffLineRow} ${tone}`}
              >
                <span className={styles.op}>{OP_GLYPH[it.op]}</span>
                <span className={styles.diffMain}>
                  <span>{it.primary}</span>
                  {it.secondary && (
                    <span className={styles.diffSecondary}>{it.secondary}</span>
                  )}
                  {it.detail && (
                    <span className={styles.detail}>{it.detail}</span>
                  )}
                </span>
                {onRevoke && (
                  // disabled 按钮不触发 title，外包一层承接悬停提示
                  <span
                    className={styles.revokeBtnWrap}
                    title={blockReason ?? undefined}
                  >
                    <button
                      type="button"
                      className={styles.revokeBtn}
                      disabled={revokePending || !!blockReason}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (blockReason) return;
                        onRevoke(it.revoke);
                      }}
                    >
                      撤销
                    </button>
                  </span>
                )}
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}

export { DiffView };
export type { DiffRevokeTarget };
