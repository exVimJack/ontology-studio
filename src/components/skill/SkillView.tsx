// UI 层：SkillView.tsx（决策 20）
// 技能管理主视图（覆盖层，同 FederationView 同级）。
//
// 定位：与「知识库」「数据源」并列为三大功能入口之一。
// 形态：卡片网格 + 来源筛选 tabs（全部/内置/已导入/外部）。
//
// 设计参考（易用性）：
// - skills-manager 的 Library 卡片网格（每卡：name + 描述 + 来源 badge + 状态 + 操作）
// - Cursor 的来源分类筛选
// - VS Code 扩展商店的卡片信息密度（icon + 标题 + 描述 + 元信息行）
// - 卡片点击展开详情（license/compatibility/allowed_tools/dmi 说明）
//
// 三层 disable 在卡片的体现：
// - 全局禁用 → 卡片置灰 + 角标「已禁用」+ 开关可解除
// - disable_model_invocation → 盾牌角标 + 详情说明 @ 手动调用
// - 激活状态 → 卡片左侧色条（绿=激活，灰=未激活）
//
// 操作：
// - 导入目录/zip（顶部工具栏）
// - 全局禁用开关（卡片右上）
// - 卸载（仅 imported，卡片操作区）
// - 查看详情（点击卡片展开）

import { useMemo, useState } from "react";
import {
  Sparkles,
  FolderUp,
  Package,
  Loader2,
  Shield,
  Trash2,
  X,
  RefreshCw,
  Download,
  ChevronDown,
  ChevronRight,
  Power,
} from "lucide-react";
import { useUiStore } from "@/stores/ui-store";
import {
  useSkillsGlobal,
  useImportSkillFromDir,
  useImportSkillFromZip,
  useUninstallSkill,
  useSetSkillGloballyDisabled,
} from "@/hooks/useSkills";
import { SKILL_SOURCE_META, isSkillActive } from "@/lib/domain";
import type { SkillDto, SkillSource } from "@/lib/domain";

type FilterTab = "all" | SkillSource;

const FILTER_TABS: { key: FilterTab; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "builtin", label: "内置" },
  { key: "imported", label: "已导入" },
  { key: "external-read-only", label: "外部" },
  { key: "project", label: "项目" },
];

export function SkillView() {
  const setSkillsOpen = useUiStore((s) => s.setSkillsOpen);
  const {
    data: skills = [],
    isLoading,
    isError,
    error,
    refetch,
  } = useSkillsGlobal();
  const importDir = useImportSkillFromDir();
  const importZip = useImportSkillFromZip();
  const uninstall = useUninstallSkill();
  const setGlobal = useSetSkillGloballyDisabled();

  const [filter, setFilter] = useState<FilterTab>("all");
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);

  const counts = useMemo(() => {
    const c: Record<string, number> = { all: skills.length };
    for (const s of skills) c[s.source] = (c[s.source] ?? 0) + 1;
    return c;
  }, [skills]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return skills.filter((s) => {
      if (filter !== "all" && s.source !== filter) return false;
      if (
        q &&
        !s.name.toLowerCase().includes(q) &&
        !s.description.toLowerCase().includes(q)
      )
        return false;
      return true;
    });
  }, [skills, filter, query]);

  const pickDir = async () => {
    const { open, message } = await import("@tauri-apps/plugin-dialog");
    const path = await open({
      directory: true,
      multiple: false,
      title: "选择 Skill 目录（含 SKILL.md）",
    });
    if (typeof path === "string") {
      importDir.mutate(path, {
        onError: async (e) =>
          message(`导入失败：${e.message}`, { kind: "error" }),
      });
    }
  };
  const pickZip = async () => {
    const { open, message } = await import("@tauri-apps/plugin-dialog");
    const path = await open({
      multiple: false,
      title: "选择 Skill zip 包",
      filters: [{ name: "zip", extensions: ["zip"] }],
    });
    if (typeof path === "string") {
      importZip.mutate(path, {
        onError: async (e) =>
          message(`导入失败：${e.message}`, { kind: "error" }),
      });
    }
  };

  const onUninstall = async (s: SkillDto) => {
    const { confirm, message } = await import("@tauri-apps/plugin-dialog");
    const ok = await confirm(
      `确定卸载技能「${s.name}」？\n将从 ~/.onto-studio/skills/ 删除目录，不可恢复。`,
      { kind: "warning" },
    );
    if (ok) {
      uninstall.mutate(s.name, {
        onError: async (e) =>
          message(`卸载失败：${e.message}`, { kind: "error" }),
      });
    }
  };

  return (
    <div className="fixed inset-0 z-40 flex flex-col bg-bg max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)]">
      {/* 顶栏 */}
      <div className="flex items-center justify-between border-b border-border px-4 py-2">
        <div className="flex items-center gap-2">
          <Sparkles size={16} className="text-accent" />
          <h1 className="text-sm font-semibold">技能</h1>
          <span className="text-xs text-fg-subtle">扩展助手能力的指令包</span>
        </div>
        <div className="flex items-center gap-1">
          <button
            onClick={() => refetch()}
            className="rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover"
            title="刷新"
          >
            <RefreshCw size={14} />
          </button>
          <button
            onClick={() => setSkillsOpen(false)}
            className="rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>
      </div>

      {/* 工具栏：筛选 tabs + 搜索 + 导入 */}
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
        {/* 来源筛选 tabs */}
        <div className="flex items-center gap-0.5 rounded-md bg-bg-elevated p-0.5">
          {FILTER_TABS.map((t) => {
            const cnt = counts[t.key] ?? 0;
            // 隐藏计数为 0 的非"全部" tab（避免噪声）
            if (t.key !== "all" && cnt === 0) return null;
            return (
              <button
                key={t.key}
                onClick={() => setFilter(t.key)}
                className={`rounded px-2 py-1 text-xs transition-colors ${
                  filter === t.key
                    ? "bg-accent text-accent-fg"
                    : "text-fg-muted hover:bg-bg-hover"
                }`}
              >
                {t.label}
                <span className="ml-1 opacity-60">{cnt}</span>
              </button>
            );
          })}
        </div>

        {/* 搜索 */}
        <div className="relative ml-auto w-48">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索技能…"
            className="w-full rounded-md border border-border bg-bg-elevated py-1 pl-2 pr-2 text-xs outline-none focus:border-accent"
          />
        </div>

        {/* 导入按钮 */}
        <button
          onClick={pickDir}
          disabled={importDir.isPending}
          className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-hover disabled:opacity-40"
        >
          {importDir.isPending ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <FolderUp size={12} />
          )}
          导入目录
        </button>
        <button
          onClick={pickZip}
          disabled={importZip.isPending}
          className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-hover disabled:opacity-40"
        >
          {importZip.isPending ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <Package size={12} />
          )}
          导入 zip
        </button>
      </div>

      {/* 卡片网格 */}
      <div className="min-h-0 flex-1 overflow-auto p-4">
        {isLoading ? (
          <div className="flex items-center justify-center gap-1.5 py-12 text-sm text-fg-subtle">
            <Loader2 size={16} className="animate-spin" /> 加载中…
          </div>
        ) : isError ? (
          <div className="flex flex-col items-center justify-center gap-2 py-12 text-center">
            <Sparkles size={32} className="text-fg-subtle" />
            <p className="text-sm text-fg-muted">技能加载失败</p>
            <p className="max-w-md text-xs text-fg-subtle">{String(error)}</p>
            <button
              onClick={() => refetch()}
              className="mt-2 rounded-md border border-border px-3 py-1 text-xs hover:bg-bg-hover"
            >
              重试
            </button>
          </div>
        ) : filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-2 py-12 text-center">
            <Sparkles size={32} className="text-fg-subtle" />
            <p className="text-sm text-fg-muted">
              {skills.length === 0 ? "未发现任何技能" : "当前筛选下无技能"}
            </p>
            <p className="text-xs text-fg-subtle">
              {skills.length === 0
                ? "内置技能随应用分发；也可导入目录或 zip 包"
                : "试试切换筛选条件或清空搜索"}
            </p>
            {skills.length === 0 && (
              <a
                href="https://agentskills.io"
                target="_blank"
                rel="noreferrer"
                className="mt-2 flex items-center gap-1 text-xs text-accent hover:underline"
              >
                <Download size={11} /> 了解如何编写自己的技能
              </a>
            )}
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
            {filtered.map((s) => (
              <SkillCard
                key={`${s.source}:${s.name}`}
                skill={s}
                expanded={expanded === `${s.source}:${s.name}`}
                onToggleExpand={() =>
                  setExpanded((prev) =>
                    prev === `${s.source}:${s.name}`
                      ? null
                      : `${s.source}:${s.name}`,
                  )
                }
                onToggleGlobal={() =>
                  setGlobal.mutate({
                    skillName: s.name,
                    disabled: !s.globally_disabled,
                  })
                }
                onUninstall={() => onUninstall(s)}
                uninstalling={uninstall.isPending}
              />
            ))}
          </div>
        )}
      </div>

      {/* 底部帮助提示 */}
      <div className="border-t border-border px-4 py-2 text-[11px] text-fg-subtle">
        启用的技能会告诉助手如何处理特定任务；助手会在需要时自动查阅技能内容。
        带盾牌标记的技能需在对话中用 <code className="font-mono">@技能名</code>{" "}
        手动召唤。 「外部」类技能来自 Claude/Cursor
        等其他工具的共享目录，自动发现、只读。
      </div>
    </div>
  );
}

// ── 单个技能卡片 ──
function SkillCard({
  skill: s,
  expanded,
  onToggleExpand,
  onToggleGlobal,
  onUninstall,
  uninstalling,
}: {
  skill: SkillDto;
  expanded: boolean;
  onToggleExpand: () => void;
  onToggleGlobal: () => void;
  onUninstall: () => void;
  uninstalling: boolean;
}) {
  const meta = SKILL_SOURCE_META[s.source];
  const active = isSkillActive(s);

  return (
    <div
      className={`group relative overflow-hidden rounded-lg border bg-bg-elevated transition-shadow hover:shadow-md ${
        s.globally_disabled
          ? "border-border opacity-60"
          : active
            ? "border-accent/40"
            : "border-border"
      }`}
    >
      {/* 左侧激活状态色条 */}
      <div
        className={`absolute left-0 top-0 h-full w-1 ${active ? "bg-emerald-500" : "bg-border"}`}
        aria-label={active ? "已激活" : "未激活"}
      />

      <div className="p-3 pl-4">
        {/* 头部：标题 + 来源 badge + dmi 盾牌 */}
        <div className="mb-1.5 flex items-start gap-2">
          <button onClick={onToggleExpand} className="min-w-0 flex-1 text-left">
            <div className="flex items-center gap-1.5">
              <span className="truncate text-sm font-medium text-fg">
                {s.name}
              </span>
              {s.disable_model_invocation && (
                <Shield
                  size={12}
                  className="shrink-0 text-amber-500"
                  aria-label="需手动召唤的技能"
                />
              )}
            </div>
          </button>
          <span
            className={`shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-medium ${meta.badgeCls}`}
          >
            {meta.label}
          </span>
        </div>

        {/* 描述 */}
        <button onClick={onToggleExpand} className="block w-full text-left">
          <p
            className="line-clamp-2 text-xs text-fg-subtle"
            title={s.description}
          >
            {s.description}
          </p>
        </button>

        {/* 元信息行 */}
        <div className="mt-2 flex items-center gap-2 text-[10px] text-fg-subtle">
          {s.license && <span className="truncate">{s.license}</span>}
          {s.compatibility && (
            <span className="truncate" title={s.compatibility}>
              · {s.compatibility}
            </span>
          )}
          {expanded ? (
            <ChevronDown size={11} className="ml-auto" />
          ) : (
            <ChevronRight size={11} className="ml-auto" />
          )}
        </div>

        {/* 展开详情 */}
        {expanded && (
          <div className="mt-2 space-y-1.5 border-t border-border pt-2 text-[11px] text-fg-subtle">
            {s.allowed_tools && s.allowed_tools.length > 0 && (
              <div>
                <span className="text-fg-muted">允许工具：</span>
                <code className="font-mono">{s.allowed_tools.join(", ")}</code>
              </div>
            )}
            {s.disable_model_invocation && (
              <div className="rounded bg-amber-500/10 px-1.5 py-1 text-amber-600 dark:text-amber-400">
                <Shield size={10} className="mr-1 inline" />
                助手不会自动使用此技能。需要时在对话中输入{" "}
                <code className="font-mono">@{s.name}</code> 手动召唤。
              </div>
            )}
            {s.globally_disabled && (
              <div className="rounded bg-rose-500/10 px-1.5 py-1 text-rose-600 dark:text-rose-400">
                已关闭。助手看不到此技能，对话中也无法临时开启。
              </div>
            )}
            {!s.disable_model_invocation && !s.globally_disabled && (
              <div className="text-fg-muted">
                {active
                  ? "已启用：助手会在相关任务中自动参考此技能。"
                  : "当前未启用。"}
              </div>
            )}
          </div>
        )}

        {/* 操作区 */}
        <div className="mt-2 flex items-center gap-1 border-t border-border pt-2">
          {/* 全局禁用开关 */}
          <button
            onClick={onToggleGlobal}
            className={`flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] ${
              s.globally_disabled
                ? "text-rose-500 hover:bg-rose-500/10"
                : "text-fg-subtle hover:bg-bg-hover"
            }`}
            title={s.globally_disabled ? "解除全局禁用" : "全局禁用"}
          >
            <Power size={11} />
            {s.globally_disabled ? "已禁用" : "启用中"}
          </button>

          {/* 卸载（仅 imported） */}
          {meta.removable && (
            <button
              onClick={onUninstall}
              disabled={uninstalling}
              className="ml-auto flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-danger disabled:opacity-40"
              title="卸载"
            >
              <Trash2 size={11} /> 卸载
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
