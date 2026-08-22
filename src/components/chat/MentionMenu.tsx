// UI 层：MentionMenu.tsx
// 输入框 `@` 触发的引用菜单 —— 路径感知的渐进式 tree（CONVERSATION-SCOPE.md §2.3）。
//
// 语义（决策 17 + CONVERSATION-SCOPE §2.3）：
//   `@` = 在当前消息文本里插入引用 token（位置语义）。
//   - @文件夹 → 插入 `@folderName`，加入 active_folders（范围标记，模型用工具按需检索）
//   - @文件   → 插入 `@fileName`，mount 到会话
//   - @数据源  → 插入 `@sourceName`，加入 active_sources
//   - @数据源表 → 插入 `@sourceName.tableName`，加入 active_sources
//
// 渐进展开（用户要的 tree 体验）：
//   - 输入 `@`            → 顶层：所有文件夹 + 所有数据源 + 根目录散文件
//   - 输入 `@曾国`         → 过滤顶层项（文件夹/数据源/文件名匹配）
//   - 输入 `@曾国藩专题/`   → 进入该文件夹，显示子文件夹 + 子文件
//   - 输入 `@ontology.`    → 进入该数据源，显示其表
//   - 选中文件夹项时，若该项有子项，可在 token 后补 `/` 继续展开（用户手动输入 `/`）
//
// 文件夹与文件统一在同一 tree（按 `/` 路径层级），不分开两组。
// 数据源按 `.` 展开（source.table）。

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useQueries } from "@tanstack/react-query";
import {
  FileText,
  Database,
  Table2,
  Folder as FolderIcon,
  ChevronRight,
  Zap,
  Shield,
  Boxes,
} from "lucide-react";
import type {
  DocumentSummaryDto,
  FolderNodeDto,
  SkillDto,
  OntologySummary,
} from "@/lib/domain";
import {
  useAllDocuments,
  useMountedDocuments,
  useMountDocument,
} from "@/hooks/useMountedDocuments";
import { useDataSources, fetchFederationSchema } from "@/hooks/useFederation";
import {
  useSkillsConversation,
  useSetSkillConversationEnabled,
} from "@/hooks/useSkills";
import {
  useActiveScope,
  useSetActiveFolders,
  useSetActiveSources,
  useSetActiveOntologies,
} from "@/hooks/useActiveScope";
import { useOntologies } from "@/hooks/useOntology";
import { useFolders } from "@/hooks/useFolders";
import { getFileIcon } from "@/lib/file-icons";
import { isSkillActive } from "@/lib/domain";

interface MentionMenuProps {
  conversationId: string;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  text: string;
  onTextChange: (next: string) => void;
}

/** 统一候选条目（tree 渲染用，所有层级共用）。
 *
 * - folder：文件夹节点（可继续 `/` 展开）；选中 = 加 active_folders + 插 token
 * - doc：文件；选中 = mount + 插 token
 * - source：数据源（可继续 `.` 展开）；选中 = 加 active_sources + 插 token
 * - table：数据源表；选中 = 加 active_sources + 插 token
 * - skill：技能（决策 20）；选中 = 即时激活（写 conversation_skills.enabled=true） + 插 `@skillName` token
 * - ontology：本体（会话引用本体）；选中 = 加 active_ontologies + 插 `@OntologyApiName` token
 */
type Candidate =
  | { kind: "folder"; path: string; name: string; hasChildren: boolean }
  | { kind: "doc"; doc: DocumentSummaryDto }
  | { kind: "source"; name: string; hasTables: boolean }
  | { kind: "table"; sourceName: string; tableName: string }
  | { kind: "skill"; skill: SkillDto }
  | { kind: "ontology"; ont: OntologySummary };

/** 选中后插入的 token。 */
function candidateToken(c: Candidate): string {
  switch (c.kind) {
    case "folder":
      return `@${c.name}`;
    case "doc":
      return `@${c.doc.name}`;
    case "source":
      return `@${c.name}`;
    case "table":
      return `@${c.sourceName}.${c.tableName}`;
    case "skill":
      return `@${c.skill.name}`;
    case "ontology":
      return `@${c.ont.api_name}`;
  }
}

/** 解析 query 决定当前展开层级。
 *  - {mode:"root"}            ：顶层（无 / 也无 .）
 *  - {mode:"folder", prefix}  ：文件夹内（query 以 prefix/ 开头或就是 prefix/）
 *  - {mode:"source", name}    ：数据源表列表（query = name. 或 name.xxx）
 */
type NavMode =
  | { mode: "root" }
  | { mode: "folder"; prefix: string }
  | { mode: "source"; sourceName: string };

function parseNav(query: string): NavMode {
  // 优先判 `.`（数据源表展开）——但仅当 query 不含 `/`（文件夹路径里可能有 . 如 "v1.2"）
  const dotIdx = query.lastIndexOf(".");
  const slashIdx = query.lastIndexOf("/");
  if (dotIdx >= 0 && dotIdx > slashIdx) {
    // query 形如 "sourceName." 或 "sourceName.xxx"
    const sourceName = query.slice(0, dotIdx);
    if (sourceName && !sourceName.includes("/") && !sourceName.includes(".")) {
      return { mode: "source", sourceName };
    }
  }
  if (slashIdx >= 0) {
    // 文件夹内：prefix = @ 后到最后一个 / 的内容（含 /）
    const prefix = query.slice(0, slashIdx + 1);
    // prefix 形如 "曾国藩专题/" 或 "曾国藩专题/书信集/"
    // 去掉末尾 / 得 folder path 加前导 /
    return { mode: "folder", prefix };
  }
  return { mode: "root" };
}

/** 从 query 提取「最后一段」用于过滤当前层级的项。
 *  root：query 本身；folder：最后一个 / 后的文本；source：最后一个 . 后的文本。 */
function filterTerm(query: string): string {
  const slashIdx = query.lastIndexOf("/");
  if (slashIdx >= 0) return query.slice(slashIdx + 1);
  const dotIdx = query.lastIndexOf(".");
  if (dotIdx >= 0) return query.slice(dotIdx + 1);
  return query;
}

export function MentionMenu({
  conversationId,
  textareaRef,
  text,
  onTextChange,
}: MentionMenuProps) {
  const { data: folderTree = [] } = useFolders();
  const { data: sources = [] } = useDataSources();
  const { data: allDocs = [] } = useAllDocuments();
  const { data: mountedDocs = [] } = useMountedDocuments(conversationId);
  const { data: skills = [] } = useSkillsConversation(conversationId);
  const { data: ontologies = [] } = useOntologies();
  const mount = useMountDocument();
  const setSkillEnabled = useSetSkillConversationEnabled(conversationId);
  const { data: scope } = useActiveScope(conversationId);
  const setActiveFolders = useSetActiveFolders();
  const setActiveSources = useSetActiveSources();
  const setActiveOntologies = useSetActiveOntologies();

  // 批量查每个数据源的表结构（菜单展开时才发）。
  const [schemaEnabled, setSchemaEnabled] = useState(false);
  const schemaQueries = useQueries({
    queries: sources.map((s) => ({
      queryKey: ["federation-schema", s.name] as const,
      queryFn: () => fetchFederationSchema(s.name),
      enabled: schemaEnabled,
      staleTime: 60_000,
    })),
  });

  const [open, setOpen] = useState(false);
  const [atIndex, setAtIndex] = useState(-1);
  const [query, setQuery] = useState("");
  const [activeIdx, setActiveIdx] = useState(0);
  const menuRef = useRef<HTMLDivElement>(null);

  const mountedPaths = useMemo(
    () => new Set(mountedDocs.map((d) => d.path)),
    [mountedDocs],
  );

  // 当前展开模式（依 query 动态）。
  const nav = useMemo(() => parseNav(query), [query]);
  const term = useMemo(() => filterTerm(query).toLowerCase(), [query]);

  // 进入文件夹模式时，拉取该文件夹直接子文件。
  const folderModePrefix = nav.mode === "folder" ? nav.prefix : null;
  // folder path = "/" + prefix 去掉末尾 /（prefix 形如 "曾国藩专题/" → "/曾国藩专题"）
  const folderPath = useMemo(() => {
    if (!folderModePrefix) return null;
    const p = "/" + folderModePrefix.replace(/\/+$/, "");
    return p === "/" ? null : p;
  }, [folderModePrefix]);

  // 在 folderTree 中按 prefix 找当前层的子文件夹节点。
  const currentFolderChildren = useMemo<FolderNodeDto[]>(() => {
    if (nav.mode !== "folder") return [];
    // prefix 形如 "曾国藩专题/" 或 "曾国藩专题/书信集/"
    const parts = nav.prefix.split("/").filter(Boolean);
    let nodes = folderTree;
    for (const part of parts) {
      const found = nodes.find((n) => n.name === part);
      if (!found) return [];
      nodes = found.children;
    }
    return nodes;
  }, [nav, folderTree]);

  // 构建当前层级的候选列表。
  const candidates = useMemo<Candidate[]>(() => {
    const list: Candidate[] = [];
    if (nav.mode === "root") {
      // 顶层：所有文件夹 + 所有数据源 + 根目录散文件（folder_path 为 NULL/空）。
      // 文件夹内的文件需 @folderName/ 进入查看（tree 展开式）。
      for (const n of folderTree) {
        list.push({
          kind: "folder",
          path: n.path,
          name: n.name,
          hasChildren: n.children.length > 0,
        });
      }
      for (const s of sources) {
        const qi = sources.findIndex((x) => x.name === s.name);
        const tables = schemaQueries[qi]?.data?.tables ?? [];
        list.push({
          kind: "source",
          name: s.name,
          hasTables: tables.length > 0,
        });
      }
      // 根目录散文件（不属于任何文件夹）
      for (const doc of allDocs) {
        const fp = doc.folder_path;
        if (!fp || fp === "/" || fp === "") {
          list.push({ kind: "doc", doc });
        }
      }
      // 可 @ 激活的 skill（全局禁用 / project 二期未启用不列出，与 resolveMentionedPaths 一致）
      // 放最后：skill 是「能力」而非「数据」，与文件夹/数据源/文件概念分组区分
      for (const s of skills) {
        if (s.globally_disabled || s.source === "project") continue;
        list.push({ kind: "skill", skill: s });
      }
      // 本体（会话引用：@OntologyApiName，加 active_ontologies）
      // 放 skill 前——本体是「结构化知识」，与数据源概念更近
      for (const o of ontologies) {
        list.push({ kind: "ontology", ont: o });
      }
    } else if (nav.mode === "folder") {
      // 文件夹内：子文件夹 + 直接子文件
      for (const n of currentFolderChildren) {
        list.push({
          kind: "folder",
          path: n.path,
          name: n.name,
          hasChildren: n.children.length > 0,
        });
      }
      // folderDocs 是 (id, name, format, char_count, path, folder_path, ...) 元组，需映射
      // 但 DocumentSummaryDto 与 listDocumentsByFolder 返回类型不同——用 allDocs 过滤更简单
      for (const doc of allDocs) {
        if (doc.folder_path === folderPath) {
          list.push({ kind: "doc", doc });
        }
      }
    } else {
      // source 模式：该源的表
      const sourceName = nav.sourceName;
      const qi = sources.findIndex((x) => x.name === sourceName);
      const tables = qi >= 0 ? (schemaQueries[qi]?.data?.tables ?? []) : [];
      for (const t of tables) {
        list.push({ kind: "table", sourceName, tableName: t.name });
      }
    }
    // 过滤
    if (!term) return list;
    return list.filter((c) => {
      const name =
        c.kind === "folder"
          ? c.name
          : c.kind === "doc"
            ? c.doc.name
            : c.kind === "source"
              ? c.name
              : c.kind === "skill"
                ? c.skill.name
                : c.kind === "ontology"
                  ? c.ont.api_name
                  : c.tableName;
      return name.toLowerCase().includes(term);
    });
  }, [
    nav,
    folderTree,
    sources,
    allDocs,
    skills,
    ontologies,
    schemaQueries,
    currentFolderChildren,
    folderPath,
    term,
  ]);

  // 监听文本 + 光标变化，更新 open/query/atIndex。
  useEffect(() => {
    const el = textareaRef.current;
    const caret = el?.selectionStart ?? 0;
    const before = text.slice(0, caret);
    const atIdx = before.lastIndexOf("@");
    if (atIdx < 0 || (atIdx > 0 && text[atIdx - 1] === "@")) {
      if (open) setOpen(false);
      return;
    }
    const q = before.slice(atIdx + 1);
    // query 内不能有空白（空白结束 @ 引用）
    if (/\s/.test(q)) {
      if (open) setOpen(false);
      return;
    }
    setAtIndex(atIdx);
    setQuery(q);
    setActiveIdx(0);
    setOpen(true);
    setSchemaEnabled(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text]);

  // 候选变化时重置高亮
  useEffect(() => {
    setActiveIdx(0);
  }, [candidates.length]);

  const select = (c: Candidate) => {
    const el = textareaRef.current;
    // 激活集侧效果（位置语义：token 只标记「在这里查」，不注入内容）。
    if (c.kind === "folder") {
      const current = scope?.folders ?? [];
      if (!current.includes(c.path)) {
        setActiveFolders.mutate({
          conversationId,
          folders: [...current, c.path],
        });
      }
    } else if (c.kind === "source") {
      const current = scope?.sources ?? [];
      if (!current.includes(c.name)) {
        setActiveSources.mutate({
          conversationId,
          sources: [...current, c.name],
        });
      }
    } else if (c.kind === "doc") {
      if (!mountedPaths.has(c.doc.path)) {
        mount.mutate({ conversationId, path: c.doc.path });
      }
    } else if (c.kind === "skill") {
      // skill 即时激活：写 conversation_skills.enabled=true。
      // disable-model-invocation 的 skill 仅靠 @ 激活后可读（后端 active_skill_doc_paths 依据会话级 enabled）。
      // 已激活则不重复写（避免不必要的 mutation + invalidate）。
      if (!isSkillActive(c.skill)) {
        setSkillEnabled.mutate({
          skillName: c.skill.name,
          source: c.skill.source,
          enabled: true,
        });
      }
    } else if (c.kind === "ontology") {
      // 本体引用：加 active_ontologies（agent 用只读工具按 api_name 钻取 schema）
      const current = scope?.ontologies ?? [];
      if (!current.includes(c.ont.api_name)) {
        setActiveOntologies.mutate({
          conversationId,
          ontologies: [...current, c.ont.api_name],
        });
      }
    } else {
      // table
      const current = scope?.sources ?? [];
      if (!current.includes(c.sourceName)) {
        setActiveSources.mutate({
          conversationId,
          sources: [...current, c.sourceName],
        });
      }
    }
    const token = candidateToken(c);
    if (!el || atIndex < 0) {
      // 无 textarea 也要更新文本（插到末尾）
      onTextChange(text + token);
      setOpen(false);
      return;
    }
    const caret = el.selectionStart ?? text.length;
    // 替换 @ + query 为 token
    const next = text.slice(0, atIndex) + token + text.slice(caret);
    onTextChange(next);
    setOpen(false);
    requestAnimationFrame(() => {
      el.focus();
      const pos = atIndex + token.length;
      el.setSelectionRange(pos, pos);
    });
  };

  // 键盘导航
  useEffect(() => {
    if (!open || candidates.length === 0) return;
    const el = textareaRef.current;
    if (!el) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setActiveIdx((i) => (i + 1) % candidates.length);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setActiveIdx((i) => (i - 1 + candidates.length) % candidates.length);
      } else if (e.key === "Enter") {
        e.preventDefault();
        const target = candidates[activeIdx];
        if (target) select(target);
      } else if (e.key === "Escape") {
        e.preventDefault();
        setOpen(false);
      }
    };
    el.addEventListener("keydown", onKey);
    return () => el.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, candidates, activeIdx, atIndex]);

  // 点击外部关闭
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        const el = textareaRef.current;
        if (el && el.contains(e.target as Node)) return;
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open, textareaRef]);

  if (!open) return null;

  // 当前层标题
  const header =
    nav.mode === "root"
      ? "知识库"
      : nav.mode === "folder"
        ? (folderPath ?? "/")
        : `${nav.sourceName} 的表`;

  return (
    <div
      ref={menuRef}
      className="absolute bottom-full left-0 z-50 mb-1 max-h-72 w-80 max-w-[calc(100vw-1.5rem)] overflow-auto rounded-md border border-border bg-bg-elevated py-1 shadow-lg"
    >
      {/* 层级标题（面包屑） */}
      <div className="border-b border-border px-3 py-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
        {nav.mode === "folder" && (
          <FolderIcon size={10} className="mr-1 inline" />
        )}
        {nav.mode === "source" && (
          <Database size={10} className="mr-1 inline" />
        )}
        {nav.mode === "root" && <FileText size={10} className="mr-1 inline" />}
        {header}
        {nav.mode === "folder" && (
          <span className="ml-1 text-fg-muted">（输入 / 进入子目录）</span>
        )}
        {nav.mode === "source" && (
          <span className="ml-1 text-fg-muted">（输入 . 选表）</span>
        )}
      </div>

      {candidates.length === 0 ? (
        <div className="px-3 py-2 text-xs text-fg-subtle">
          {nav.mode === "folder"
            ? "此目录无子项"
            : nav.mode === "source"
              ? "此数据源无表，或表清单加载中…"
              : "暂无可引用内容"}
        </div>
      ) : (
        candidates.map((c, i) => {
          const isActive = activeIdx === i;
          // 分组标题：每个类别首项前插分隔标题（文件夹/数据源/文件/本体/技能）
          // 文件夹不插标题（顶层默认上下文就是文件夹树）；其余插标题区分。
          const prevKind = i === 0 ? null : (candidates[i - 1]?.kind ?? null);
          const header =
            c.kind === "source" && prevKind !== "source"
              ? { icon: Database, label: "数据源" }
              : c.kind === "doc" && prevKind !== "doc"
                ? { icon: FileText, label: "文件" }
                : c.kind === "ontology" && prevKind !== "ontology"
                  ? { icon: Boxes, label: "本体" }
                  : c.kind === "skill" && prevKind !== "skill"
                    ? { icon: Zap, label: "技能" }
                    : null;
          return (
            <Fragment
              key={
                c.kind === "folder"
                  ? `fld-${c.path}`
                  : c.kind === "doc"
                    ? `doc-${c.doc.id}`
                    : c.kind === "source"
                      ? `src-${c.name}`
                      : c.kind === "skill"
                        ? `skl-${c.skill.source}-${c.skill.name}`
                        : c.kind === "ontology"
                          ? `ont-${c.ont.api_name}`
                          : `tbl-${c.sourceName}-${c.tableName}`
              }
            >
              {header && (
                <div className="flex items-center gap-1 border-t border-border px-3 py-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                  <header.icon size={10} className="inline" />
                  {header.label}
                </div>
              )}
              <button
                onClick={() => select(c)}
                onMouseEnter={() => setActiveIdx(i)}
                className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs ${
                  isActive ? "bg-bg-hover" : ""
                }`}
              >
                {c.kind === "folder" && (
                  <>
                    <FolderIcon size={14} className="shrink-0 text-accent/70" />
                    <span className="min-w-0 flex-1 truncate text-fg">
                      {c.name}
                    </span>
                    {c.hasChildren && (
                      <ChevronRight
                        size={12}
                        className="shrink-0 text-fg-subtle"
                      />
                    )}
                  </>
                )}
                {c.kind === "doc" && (
                  <>
                    {(() => {
                      const { Icon, className: iconCls } = getFileIcon(
                        c.doc.format,
                        c.doc.name,
                      );
                      return (
                        <Icon size={14} className={`shrink-0 ${iconCls}`} />
                      );
                    })()}
                    <span className="min-w-0 flex-1 truncate">
                      {c.doc.name}
                    </span>
                    {c.doc.char_count > 0 && (
                      <span className="shrink-0 text-[10px] text-fg-subtle">
                        {c.doc.char_count > 1000
                          ? `${Math.round(c.doc.char_count / 1000)}k`
                          : c.doc.char_count}
                      </span>
                    )}
                  </>
                )}
                {c.kind === "source" && (
                  <>
                    <Database size={14} className="shrink-0 text-accent/70" />
                    <span className="min-w-0 flex-1 truncate text-fg">
                      {c.name}
                    </span>
                    {c.hasTables && (
                      <ChevronRight
                        size={12}
                        className="shrink-0 text-fg-subtle"
                      />
                    )}
                  </>
                )}
                {c.kind === "table" && (
                  <>
                    <Table2 size={14} className="shrink-0 text-fg-subtle" />
                    <span className="min-w-0 flex-1 truncate text-fg">
                      {c.tableName}
                    </span>
                  </>
                )}
                {c.kind === "ontology" && (
                  <>
                    <Boxes size={14} className="shrink-0 text-accent/70" />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1">
                        <span className="truncate text-fg">
                          {c.ont.display_name}
                        </span>
                        <span className="shrink-0 rounded bg-bg-hover px-1 text-[9px] text-fg-subtle">
                          {c.ont.api_name}
                        </span>
                        {scope?.ontologies?.includes(c.ont.api_name) && (
                          <span className="shrink-0 rounded bg-accent/10 px-1 text-[9px] text-accent">
                            已引用
                          </span>
                        )}
                      </div>
                      {c.ont.description && (
                        <p
                          className="truncate text-[10px] text-fg-subtle"
                          title={c.ont.description}
                        >
                          {c.ont.description}
                        </p>
                      )}
                    </div>
                  </>
                )}
                {c.kind === "skill" && (
                  <>
                    <Zap size={14} className="shrink-0 text-amber-500" />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1">
                        <span className="truncate text-fg">{c.skill.name}</span>
                        {c.skill.disable_model_invocation && (
                          <Shield
                            size={9}
                            className="shrink-0 text-amber-500"
                            aria-label="需 @ 手动激活"
                          />
                        )}
                        {isSkillActive(c.skill) && (
                          <span className="shrink-0 rounded bg-accent/10 px-1 text-[9px] text-accent">
                            已激活
                          </span>
                        )}
                      </div>
                      <p
                        className="truncate text-[10px] text-fg-subtle"
                        title={c.skill.description}
                      >
                        {c.skill.description}
                      </p>
                    </div>
                  </>
                )}
              </button>
            </Fragment>
          );
        })
      )}
    </div>
  );
}
