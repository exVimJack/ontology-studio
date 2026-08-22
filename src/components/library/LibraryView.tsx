// UI 层：LibraryView.tsx（CONVERSATION-SCOPE.md §5.1）
//
// 知识库文件树视图（云盘模式）：左侧文件夹列表 + 右侧选中文件夹的文件。
// folders 表持久化空文件夹 + documents.folder_path 双轨（决策 19）。
// 支持：新建文件夹、多选移动（Dialog 选目标）、重命名/删除文件夹。
//
// 数据源：useFolders（folders 表 ∪ DISTINCT folder_path）+ useDocumentsByFolder（指定文件夹下文件）。
//
// 每项操作：
//   - 挂载到当前会话：mountDocument(currentConvId, path)（仅 chat 路由可用）
//   - 预览：readDocument(id) 读全文抽屉展示
//   - 删除：deleteDocument(path)
//   - 移动文件：moveDocument(path, targetFolder)
//   - 重命名文件夹：renameFolder(oldPath, newPath)
//   - 删除文件夹：deleteFolder(folder)（级联删所有文件）

import { useEffect, useMemo, useState } from "react";
import {
  Search,
  Trash2,
  Link2,
  Folder as FolderIcon,
  Eye,
  X,
  Inbox,
  ChevronRight,
  Plus,
  CheckSquare,
} from "lucide-react";
import { confirm, message } from "@tauri-apps/plugin-dialog";
import type { DocumentSummaryDto } from "@/lib/domain";
import { useCurrentConversationId } from "@/hooks/useCurrentConversationId";
import { useIsMobile } from "@/hooks/useIsMobile";
import { useUiStore } from "@/stores/ui-store";
import {
  useDocumentContent,
  useDeleteDocument,
  useMountedDocuments,
  useMountDocument,
} from "@/hooks/useMountedDocuments";
import {
  useFolders,
  useDocumentsByFolder,
  useCreateFolder,
  useMoveDocument,
  useRenameFolder,
  useDeleteFolder,
} from "@/hooks/useFolders";
import { getFileIcon } from "@/lib/file-icons";
import { formatCharCount } from "@/lib/format";
import { IngestStatusBoard } from "@/components/library/IngestStatusBoard";
import type { FolderNodeDto } from "@/lib/domain";

function FolderTreeItem({
  node,
  depth,
  selected,
  onSelect,
}: {
  node: FolderNodeDto;
  depth: number;
  selected: string | null;
  onSelect: (path: string) => void;
}) {
  const isSel = selected === node.path;
  return (
    <>
      <button
        onClick={() => onSelect(node.path)}
        className={`flex w-full items-center gap-1 rounded-md py-1 pr-2 text-left text-xs ${
          isSel ? "bg-accent/15 text-accent" : "text-fg hover:bg-bg-hover"
        }`}
        style={{ paddingLeft: depth * 12 + 8 }}
      >
        <ChevronRight
          size={10}
          className={`shrink-0 text-fg-subtle ${isSel ? "rotate-90" : ""}`}
        />
        <FolderIcon
          size={12}
          className={`shrink-0 ${isSel ? "text-accent" : "text-fg-subtle"}`}
        />
        <span className="truncate">{node.name}</span>
      </button>
      {node.children.map((c) => (
        <FolderTreeItem
          key={c.path}
          node={c}
          depth={depth + 1}
          selected={selected}
          onSelect={onSelect}
        />
      ))}
    </>
  );
}

export function LibraryView() {
  const { data: tree = [] } = useFolders();
  const mount = useMountDocument();
  const del = useDeleteDocument();
  const moveDoc = useMoveDocument();
  const createFld = useCreateFolder();
  const renameFld = useRenameFolder();
  const deleteFld = useDeleteFolder();
  const currentConvId = useCurrentConversationId();
  const { data: mountedDocs = [] } = useMountedDocuments(currentConvId);
  const isMobile = useIsMobile();

  const [selectedFolder, setSelectedFolder] = useState<string | null>(null);
  // 移动端视图切换：'tree' 显示文件夹树，'files' 显示当前文件夹文件列表
  // 桌面端两栏同屏，不使用此状态
  const [mobileView, setMobileView] = useState<"tree" | "files">("tree");
  const selectFolder = (path: string | null) => {
    setSelectedFolder(path);
    if (isMobile) setMobileView("files");
  };
  const { data: folderDocs = [] } = useDocumentsByFolder(selectedFolder);
  const setLibraryUploadFolder = useUiStore((s) => s.setLibraryUploadFolder);
  const setLibraryViewActive = useUiStore((s) => s.setLibraryViewActive);

  // 选中文件夹变化时同步到全局 store（FileDropZone 上传时读取）
  useEffect(() => {
    setLibraryUploadFolder(selectedFolder);
  }, [selectedFolder, setLibraryUploadFolder]);
  // 组件挂载/卸载时标记 Library 视图激活状态（会话页上传忽略此值）
  useEffect(() => {
    setLibraryViewActive(true);
    return () => setLibraryViewActive(false);
  }, [setLibraryViewActive]);
  const [query, setQuery] = useState("");
  // 新建文件夹内联输入条（替代原生 prompt()，Tauri WebView 不支持 prompt）
  const [creating, setCreating] = useState(false);
  const [createValue, setCreateValue] = useState("");
  const [previewId, setPreviewId] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  // 多选移动（方案 A）：选中文件 path 集合 + 移动对话框目标
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  const [moveTarget, setMoveTarget] = useState<{
    paths: string[];
    target: string | null;
  } | null>(null);

  // tree 已是后端构建好的嵌套树（FolderNodeDto[]），无需前端再解析。

  const mountedPaths = useMemo(
    () => new Set(mountedDocs.map((d) => d.path)),
    [mountedDocs],
  );

  const filteredDocs = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return folderDocs;
    return folderDocs.filter((it) => it.name.toLowerCase().includes(q));
  }, [folderDocs, query]);

  const handleMount = (it: DocumentSummaryDto) => {
    if (!currentConvId) return;
    mount.mutate({ conversationId: currentConvId, path: it.path });
  };

  const handleDelete = async (it: DocumentSummaryDto) => {
    const ok = await confirm(
      `确定删除「${it.name}」？此操作不可撤销（全文 + 索引 + 所有会话挂载都会清除）。`,
      { kind: "warning" },
    );
    if (!ok) return;
    del.mutate(it.path);
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      next.delete(it.path);
      return next;
    });
  };

  // 批量删除选中文件
  const handleDeleteSelected = async () => {
    const paths = Array.from(selectedPaths);
    if (paths.length === 0) return;
    const ok = await confirm(
      `确定删除选中的 ${paths.length} 个文件？此操作不可撤销。`,
      { kind: "warning" },
    );
    if (!ok) return;
    for (const p of paths) {
      del.mutate(p);
    }
    setSelectedPaths(new Set());
  };

  // 切换选中文件（多选）
  const toggleSelect = (path: string) => {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };
  // 切换全选/全不选（当前过滤后的文件）
  const toggleSelectAll = () => {
    setSelectedPaths((prev) => {
      const allSelected = filteredDocs.every((d) => prev.has(d.path));
      if (allSelected) {
        const next = new Set(prev);
        filteredDocs.forEach((d) => next.delete(d.path));
        return next;
      } else {
        const next = new Set(prev);
        filteredDocs.forEach((d) => next.add(d.path));
        return next;
      }
    });
  };
  const handleDeleteFolder = async (folder: string) => {
    const ok = await confirm(
      `确定删除文件夹「${folder}」及其下所有文件？此操作不可撤销。`,
      { kind: "warning" },
    );
    if (!ok) return;
    deleteFld.mutate(folder);
    if (selectedFolder === folder) setSelectedFolder(null);
  };

  const handleRenameFolder = (folder: string) => {
    const newPath = renameValue.trim();
    if (!newPath || newPath === folder) {
      setRenaming(null);
      return;
    }
    renameFld.mutate(
      { oldPath: folder, newPath },
      {
        onSettled: () => {
          setRenaming(null);
          if (selectedFolder === folder) setSelectedFolder(newPath);
        },
      },
    );
  };

  const handleCreateFolder = async (name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    // 在当前选中文件夹下创建子目录；未选中则在根目录下创建
    const parent = selectedFolder ?? "";
    const path = parent
      ? `${parent}/${trimmed}`.replace(/\/+/g, "/")
      : trimmed.startsWith("/")
        ? trimmed
        : "/" + trimmed;
    createFld.mutate(path, {
      onSuccess: async (created) => {
        selectFolder(path);
        if (!created) {
          await message(`文件夹「${path}」已存在`, { kind: "info" });
        }
      },
      onError: async (e) => message(`新建文件夹失败：${e}`, { kind: "error" }),
    });
  };

  return (
    <div className="flex h-full flex-col bg-bg">
      <IngestStatusBoard />

      <div className="flex min-h-0 flex-1">
        {/* 左栏：文件夹树（移动端：tree 视图时全宽显示） */}
        <div
          className={`flex shrink-0 flex-col border-r border-border ${isMobile ? (mobileView === "tree" ? "w-full border-r-0" : "hidden") : "w-56"}`}
        >
          {isMobile && mobileView === "tree" ? (
            <div className="flex items-center gap-2 border-b border-border px-3 py-2">
              <span className="text-xs font-medium text-fg">文件夹</span>
            </div>
          ) : null}
          <div className="flex items-center justify-between px-3 py-2">
            <span className="text-xs font-medium text-fg">文件夹</span>
            <button
              onClick={() => {
                setCreating(true);
                setCreateValue("");
              }}
              title="新建文件夹"
              className="rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-accent"
            >
              <Plus size={14} />
            </button>
          </div>
          {creating && (
            <div className="flex items-center gap-2 border-b border-border px-3 py-1.5">
              <input
                value={createValue}
                onChange={(e) => setCreateValue(e.target.value)}
                placeholder={
                  selectedFolder
                    ? `在「${selectedFolder}」下新建子文件夹`
                    : "新建文件夹名称（如：曾国藩专题）"
                }
                autoFocus
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    handleCreateFolder(createValue);
                    setCreating(false);
                  } else if (e.key === "Escape") {
                    setCreating(false);
                  }
                }}
                className="flex-1 rounded border border-border bg-bg-elevated px-2 py-1 text-xs outline-none focus:border-accent"
              />
              <button
                onClick={() => {
                  handleCreateFolder(createValue);
                  setCreating(false);
                }}
                className="rounded bg-accent px-2 py-1 text-xs text-white"
              >
                确定
              </button>
              <button
                onClick={() => setCreating(false)}
                className="rounded px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover"
              >
                取消
              </button>
            </div>
          )}
          <div className="min-h-0 flex-1 overflow-auto px-2 pb-2">
            {/* 根目录散文件 */}
            <button
              onClick={() => selectFolder(null)}
              className={`flex w-full items-center gap-1 rounded-md py-1 px-2 text-left text-xs ${
                selectedFolder === null
                  ? "bg-accent/15 text-accent"
                  : "text-fg hover:bg-bg-hover"
              }`}
            >
              <FolderIcon size={12} className="shrink-0 text-fg-subtle" />
              <span>根目录</span>
            </button>
            {tree.map((node) => (
              <FolderTreeItem
                key={node.path}
                node={node}
                depth={0}
                selected={selectedFolder}
                onSelect={selectFolder}
              />
            ))}
          </div>
        </div>

        {/* 右栏：选中文件夹的文件（移动端：files 视图时全宽显示） */}
        <div
          className={`flex min-w-0 flex-1 flex-col ${isMobile && mobileView === "tree" ? "hidden" : "flex"}`}
        >
          {/* 移动端返回按钮 */}
          {isMobile && (
            <button
              onClick={() => setMobileView("tree")}
              className="flex items-center gap-1.5 border-b border-border px-3 py-2 text-xs text-fg-muted hover:bg-bg-hover"
            >
              <ChevronRight size={14} className="rotate-180" />
              返回文件夹
            </button>
          )}
          {/* 工具栏 */}
          <div className="flex items-center gap-2 border-b border-border px-3 py-2">
            <span className="truncate text-xs text-fg-muted">
              {selectedFolder ?? "根目录"}
            </span>
            {selectedFolder && (
              <>
                <button
                  onClick={() => {
                    setRenaming(selectedFolder);
                    setRenameValue(selectedFolder);
                  }}
                  className="rounded px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-accent"
                >
                  重命名
                </button>
                <button
                  onClick={() => handleDeleteFolder(selectedFolder)}
                  className="rounded px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-danger"
                >
                  删除文件夹
                </button>
              </>
            )}
            {selectedPaths.size > 0 && (
              <button
                onClick={() =>
                  setMoveTarget({
                    paths: Array.from(selectedPaths),
                    target: selectedFolder,
                  })
                }
                className="rounded-md bg-accent px-2 py-1 text-xs text-white hover:bg-accent/90"
              >
                移动 {selectedPaths.size} 项
              </button>
            )}
            {selectedPaths.size > 0 && (
              <button
                onClick={() => setSelectedPaths(new Set())}
                className="rounded px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover"
              >
                取消选择
              </button>
            )}
            {selectedPaths.size > 0 && (
              <button
                onClick={handleDeleteSelected}
                className="rounded-md px-2 py-1 text-xs text-danger hover:bg-danger/10"
              >
                删除 {selectedPaths.size} 项
              </button>
            )}
            <div className="relative ml-auto">
              <Search
                size={13}
                className="absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
              />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="搜索文件名…"
                className="w-40 rounded-md border border-border bg-bg-elevated py-1 pl-7 pr-2 text-xs outline-none focus:border-accent"
              />
            </div>
          </div>

          {/* 重命名输入条 */}
          {renaming && (
            <div className="flex items-center gap-2 border-b border-border px-3 py-1.5">
              <input
                value={renameValue}
                onChange={(e) => setRenameValue(e.target.value)}
                autoFocus
                className="flex-1 rounded border border-border bg-bg-elevated px-2 py-1 text-xs outline-none focus:border-accent"
              />
              <button
                onClick={() => handleRenameFolder(renaming)}
                className="rounded bg-accent px-2 py-1 text-xs text-white"
              >
                确定
              </button>
              <button
                onClick={() => setRenaming(null)}
                className="rounded px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover"
              >
                取消
              </button>
            </div>
          )}

          {/* 文件列表 */}
          <div className="flex-1 overflow-auto">
            {filteredDocs.length === 0 ? (
              <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
                <Inbox size={28} className="text-fg-subtle" />
                <p className="text-sm text-fg-muted">
                  {selectedFolder ? "此文件夹为空" : "根目录无散文件"}
                </p>
                <p className="text-xs text-fg-subtle">
                  {selectedFolder
                    ? "在对话中上传文件时会落入 /Inbox，可在此移动到此文件夹"
                    : "未归入任何文件夹的文件会显示在这里"}
                </p>
              </div>
            ) : (
              <ul className="divide-y divide-border">
                {/* 表头：全选 checkbox */}
                <li className="flex items-center gap-3 border-b border-border bg-bg-elevated px-3 py-1.5">
                  <button
                    onClick={toggleSelectAll}
                    title="全选/取消"
                    className="text-fg-subtle hover:text-accent"
                  >
                    <CheckSquare
                      size={15}
                      className={
                        filteredDocs.every((d) => selectedPaths.has(d.path)) &&
                        filteredDocs.length > 0
                          ? "text-accent"
                          : "text-fg-subtle"
                      }
                    />
                  </button>
                  <span className="text-[11px] text-fg-subtle">
                    {filteredDocs.length} 个文件
                    {selectedPaths.size > 0 && ` · 已选 ${selectedPaths.size}`}
                  </span>
                </li>
                {filteredDocs.map((it) => {
                  const { Icon, className: iconCls } = getFileIcon(
                    it.format,
                    it.name,
                  );
                  const isMounted = mountedPaths.has(it.path);
                  const isChecked = selectedPaths.has(it.path);
                  return (
                    <li
                      key={it.id}
                      className={`group flex items-center gap-3 px-3 py-2 hover:bg-bg-hover ${
                        isChecked ? "bg-accent/5" : ""
                      }`}
                    >
                      {/* 选中 checkbox */}
                      <button
                        onClick={() => toggleSelect(it.path)}
                        className="shrink-0"
                        title={isChecked ? "取消选择" : "选择"}
                      >
                        <CheckSquare
                          size={15}
                          className={
                            isChecked
                              ? "text-accent"
                              : "text-fg-subtle/50 hover:text-fg-subtle"
                          }
                        />
                      </button>
                      <Icon size={18} className={`shrink-0 ${iconCls}`} />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="truncate text-sm text-fg">
                            {it.name}
                          </span>
                          {isMounted && currentConvId && (
                            <span className="shrink-0 rounded bg-accent/15 px-1 text-[10px] text-accent">
                              已挂载
                            </span>
                          )}
                        </div>
                        <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
                          <span className="uppercase">{it.format}</span>
                          {it.char_count > 0 && (
                            <span>· {formatCharCount(it.char_count)}</span>
                          )}
                        </div>
                      </div>
                      {/* 操作按钮 */}
                      <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                        <button
                          onClick={() => setPreviewId(it.id)}
                          title="预览"
                          className="rounded p-1 text-fg-subtle hover:bg-bg hover:text-accent"
                        >
                          <Eye size={14} />
                        </button>
                        <button
                          onClick={() => handleMount(it)}
                          disabled={!currentConvId || isMounted}
                          title={
                            !currentConvId
                              ? "请先打开一个会话"
                              : isMounted
                                ? "已挂载到当前会话"
                                : "挂载到当前会话"
                          }
                          className="rounded p-1 text-fg-subtle hover:bg-bg hover:text-accent disabled:opacity-30"
                        >
                          <Link2 size={14} />
                        </button>
                        {/* 移动到文件夹：打开选目标对话框 */}
                        <button
                          onClick={() =>
                            setMoveTarget({
                              paths: [it.path],
                              target: it.folder_path ?? null,
                            })
                          }
                          title="移动到文件夹"
                          className="rounded p-1 text-fg-subtle hover:bg-bg hover:text-accent"
                        >
                          <FolderIcon size={14} />
                        </button>
                        <button
                          onClick={() => handleDelete(it)}
                          title="删除（从知识库移除全文 + 索引 + 所有会话挂载）"
                          className="rounded p-1 text-fg-subtle hover:bg-bg hover:text-danger"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </div>
      </div>

      {/* 预览抽屉 */}
      {previewId && (
        <PreviewDrawer id={previewId} onClose={() => setPreviewId(null)} />
      )}

      {/* 移动文件对话框 */}
      {moveTarget && (
        <MoveDialog
          paths={moveTarget.paths}
          initialTarget={moveTarget.target}
          tree={tree}
          onConfirm={(target) => {
            setMoveTarget((prev) => (prev ? { ...prev, target } : prev));
            // 立即执行移动
            for (const p of moveTarget.paths) {
              moveDoc.mutate({ path: p, targetFolder: target });
            }
            setMoveTarget(null);
            setSelectedPaths(new Set());
          }}
          onClose={() => setMoveTarget(null)}
        />
      )}
    </div>
  );
}

// ───────── 预览抽屉 ─────────
function PreviewDrawer({ id, onClose }: { id: string; onClose: () => void }) {
  const { data: doc, isLoading } = useDocumentContent(id);

  const name = doc?.name ?? "";
  const format = doc?.format ?? "";
  const charCount = doc?.char_count ?? 0;
  const { Icon, className: iconCls } = getFileIcon(format, name);

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/30" onClick={onClose} />
      <div className="relative flex h-full w-full max-w-2xl flex-col bg-bg-elevated shadow-2xl">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <Icon size={16} className={`shrink-0 ${iconCls}`} />
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium text-fg">
              {name || "加载中…"}
            </div>
            <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
              <span className="uppercase">{format}</span>
              {charCount > 0 && <span>· {formatCharCount(charCount)}</span>}
            </div>
          </div>
          <button
            onClick={onClose}
            title="关闭"
            className="rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-fg"
          >
            <X size={16} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {isLoading ? (
            <div className="flex h-full items-center justify-center text-sm text-fg-subtle">
              加载中…
            </div>
          ) : doc?.text ? (
            <pre className="whitespace-pre-wrap break-words p-4 font-sans text-[13px] leading-relaxed text-fg">
              {doc.text}
            </pre>
          ) : (
            <div className="flex h-full items-center justify-center text-sm text-fg-subtle">
              无可预览文本
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ───────── 移动文件对话框 ─────────
// 列出所有文件夹（含根目录）供单选目标，确定后批量移动。
// 替代旧版 prompt() 手输路径——避免拼错、可视化选目标。
function MoveDialog({
  paths,
  initialTarget,
  tree,
  onConfirm,
  onClose,
}: {
  paths: string[];
  initialTarget: string | null;
  tree: FolderNodeDto[];
  onConfirm: (target: string | null) => void;
  onClose: () => void;
}) {
  const [target, setTarget] = useState<string | null>(initialTarget);

  // 扁平化文件夹树为路径列表（用于渲染 radio 列表）
  const flatPaths = useMemo(() => {
    const out: { path: string; depth: number; name: string }[] = [];
    const walk = (nodes: FolderNodeDto[], depth: number) => {
      for (const n of nodes) {
        out.push({ path: n.path, depth, name: n.name });
        walk(n.children, depth + 1);
      }
    };
    walk(tree, 0);
    return out;
  }, [tree]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/30" onClick={onClose} />
      <div className="relative flex max-h-[70vh] w-full max-w-md flex-col rounded-lg border border-border bg-bg-elevated shadow-2xl">
        {/* 标题 */}
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <FolderIcon size={15} className="text-accent" />
            <span className="text-sm font-medium text-fg">
              移动 {paths.length} 个文件到…
            </span>
          </div>
          <button
            onClick={onClose}
            title="关闭"
            className="rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-fg"
          >
            <X size={15} />
          </button>
        </div>

        {/* 文件夹列表（含根目录） */}
        <div className="min-h-0 flex-1 overflow-auto py-1">
          {/* 根目录选项 */}
          <button
            onClick={() => setTarget(null)}
            className={`flex w-full items-center gap-2 px-4 py-1.5 text-left text-xs ${
              target === null
                ? "bg-accent/10 text-accent"
                : "text-fg hover:bg-bg-hover"
            }`}
          >
            <span
              className={`h-3 w-3 shrink-0 rounded-full border ${
                target === null ? "border-accent bg-accent" : "border-fg-subtle"
              }`}
            />
            <Inbox size={13} className="shrink-0 text-fg-subtle" />
            <span>根目录（散文件）</span>
          </button>
          {flatPaths.map((f) => (
            <button
              key={f.path}
              onClick={() => setTarget(f.path)}
              style={{ paddingLeft: f.depth * 14 + 16 }}
              className={`flex w-full items-center gap-2 py-1.5 pr-4 text-left text-xs ${
                target === f.path
                  ? "bg-accent/10 text-accent"
                  : "text-fg hover:bg-bg-hover"
              }`}
            >
              <span
                className={`h-3 w-3 shrink-0 rounded-full border ${
                  target === f.path
                    ? "border-accent bg-accent"
                    : "border-fg-subtle"
                }`}
              />
              <FolderIcon size={13} className="shrink-0 text-fg-subtle" />
              <span className="truncate">{f.name}</span>
              <span className="ml-auto shrink-0 text-[10px] text-fg-subtle">
                {f.path}
              </span>
            </button>
          ))}
        </div>

        {/* 底部操作 */}
        <div className="flex items-center justify-between border-t border-border px-4 py-3">
          <span className="text-[11px] text-fg-subtle">
            目标：{target ?? "根目录"}
          </span>
          <div className="flex gap-2">
            <button
              onClick={onClose}
              className="rounded px-3 py-1.5 text-xs text-fg-subtle hover:bg-bg-hover"
            >
              取消
            </button>
            <button
              onClick={() => onConfirm(target)}
              className="rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent/90"
            >
              确定
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
