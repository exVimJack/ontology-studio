// UI 层：Composer.tsx（§20.6）
// 输入框 + 发送/中断 + 文件选择(⌘O) + 图片粘贴。
// 回车换行，⌘↵ 发送。草稿按会话持久化。
// 状态联动：流式中禁用发送显示中断；未配 provider 禁用并提示。
//
// 上下文接入（P0/P1）：
//   - 文档（PDF/Office/文本/...）：走 ingest → store → context_texts
//   - 图片：Composer 直接读文件转 base64 → context_images（一期不进 ingest 管道）

import { useEffect, useRef, useState } from "react";
import {
  Square,
  Settings,
  Paperclip,
  X,
  AlertTriangle,
  Brain,
  AtSign,
  ArrowUp,
} from "lucide-react";
import { SkillPopover } from "@/components/chat/SkillPopover";
import { open } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";
import { useHasProvider, useProvider } from "@/hooks/useProvider";
import { useIngest } from "@/hooks/useIngest";
import { useChat } from "@/hooks/useChat";
import { useMountedDocuments } from "@/hooks/useMountedDocuments";
import { useIsMobile } from "@/hooks/useIsMobile";
import {
  useSkillsConversation,
  useSetSkillConversationEnabled,
} from "@/hooks/useSkills";
import { useComposerStore } from "@/stores/composer-store";
import { useUiStore } from "@/stores/ui-store";
import { MentionMenu } from "@/components/chat/MentionMenu";
import { getFileIcon } from "@/lib/file-icons";
import {
  resolveMentionedPaths,
  resolveMentionedSkillNames,
} from "@/lib/mention";
import { isSkillActive } from "@/lib/domain";
import { resizeImageToBase64 } from "@/lib/image-resize";
import { validateImageSize } from "@/lib/context-budget";

/** 文档类扩展名（走 ingest 管道，提取文本）。与后端 dispatcher 的解析路由对齐。 */
const DOC_EXTENSIONS = [
  // PDF / Office / eBook
  "pdf",
  "docx",
  "doc",
  "pptx",
  "ppt",
  "xlsx",
  "xls",
  "epub",
  // 文本 / Markdown / 日志
  "txt",
  "md",
  "markdown",
  "log",
  // 代码
  "rs",
  "go",
  "py",
  "js",
  "ts",
  "tsx",
  "jsx",
  "java",
  "c",
  "cpp",
  "h",
  "hpp",
  "cs",
  "rb",
  "php",
  "swift",
  "kt",
  // 脚本
  "sh",
  "bash",
  "zsh",
  "fish",
  "ps1",
  "bat",
  "cmd",
  // 配置 / 标记
  "toml",
  "yaml",
  "yml",
  "ini",
  "cfg",
  "conf",
  "xml",
  "html",
  "css",
  "scss",
  "sql",
  "env",
  "properties",
  "editorconfig",
  "diff",
  "patch",
  "lock",
  // 其他纯文本语言 / 文档（TextParser 直接读 UTF-8）
  "lua",
  "scala",
  "dart",
  "r",
  "m",
  "mm",
  "pl",
  "erl",
  "ex",
  "exs",
  "clj",
  "cljs",
  "cljc",
  "fs",
  "fsx",
  "groovy",
  "gradle",
  "v",
  "vhdl",
  "sv",
  "asm",
  "s",
  "zig",
  "nim",
  "jl",
  "sol",
  "vue",
  "svelte",
  "astro",
  "less",
  "styl",
  "tex",
  "latex",
  "graphql",
  "gql",
  "proto",
  "rst",
  "adoc",
  "asciidoc",
  "org",
  "textile",
  "svg",
  "rss",
  "atom",
  // 表格 / JSON / 数据流
  "csv",
  "tsv",
  "json",
  "jsonl",
  "ndjson",
  // 压缩包 / 单文件 gzip
  "zip",
  "tar",
  "tgz",
  "tbz2",
  "txz",
  "gz",
];

/** 图片类扩展名（一期直接读 base64 → VLM，不进 ingest）。 */
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];

const ALL_EXTENSIONS = [...DOC_EXTENSIONS, ...IMAGE_EXTENSIONS];

/** 已选中的待发送图片（前端临时态，不进 ingest-store）。 */
interface PendingImage {
  id: string;
  fileName: string;
  mime: string;
  dataB64: string;
}

export function Composer({ conversationId }: { conversationId: string }) {
  const hasProvider = useHasProvider();
  const { data: providerCfg } = useProvider();
  const ingest = useIngest(conversationId);
  const chat = useChat(conversationId);
  const text = useComposerStore((s) => s.drafts[conversationId] ?? "");
  const setDraft = useComposerStore((s) => s.setDraft);
  const clearDraft = useComposerStore((s) => s.clearDraft);
  const reasoningEnabled = useComposerStore(
    (s) => s.reasoningEnabled[conversationId] ?? false,
  );
  const setReasoningEnabled = useComposerStore((s) => s.setReasoningEnabled);
  const setPaletteOpen = useUiStore((s) => s.setCommandPaletteOpen);

  // 待发送图片（一期不入 ingest-store，随消息发送后清空）
  const [pendingImages, setPendingImages] = useState<PendingImage[]>([]);
  // 图片处理提示（降采样/超限跳过等），非空时在输入框上方展示内联提示条
  const [notice, setNotice] = useState<string | null>(null);
  // 挂载文档列表（后端持久化，切走会话不丢）。发送时传 path 列表，后端读全文。
  const { data: mountedDocs = [] } = useMountedDocuments(conversationId);
  // 可用 skill 列表（会话级）。@skillName 解析为 skill://<name> path，后端按 path 查 documents 表
  // 取 skill body（与文件全文统一 read_document 路径，决策 20）。
  const { data: skills = [] } = useSkillsConversation(conversationId);
  const setSkillEnabled = useSetSkillConversationEnabled(conversationId);

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // 自动调整高度
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  }, [text]);

  // ⌘O 文件选择（全局监听，仅本会话输入区聚焦时也生效）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key.toLowerCase() === "o") {
        e.preventDefault();
        pickFiles();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // pickFiles 在闭包内，依赖下方定义；eslint 暂忽略
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationId, hasProvider]);

  /** 在 textarea 当前光标处插入 `@`，触发 MentionMenu。 */
  const insertAt = () => {
    const el = textareaRef.current;
    if (!el) return;
    const start = el.selectionStart ?? text.length;
    const end = el.selectionEnd ?? text.length;
    // 若前一个字符已是 @ 或非空白，先补一个空格避免触发检测失效
    const needSpace =
      start > 0 && text[start - 1] !== "@" && !/\s/.test(text[start - 1]);
    const insert = needSpace ? " @" : "@";
    const next = text.slice(0, start) + insert + text.slice(end);
    setDraft(conversationId, next);
    // 光标移到 @ 后，触发 MentionMenu 的 detect
    requestAnimationFrame(() => {
      el.focus();
      const pos = start + insert.length;
      el.setSelectionRange(pos, pos);
    });
  };

  const send = async () => {
    const content = text.trim();
    if (!content || chat.isSending || !hasProvider) return;

    // 收集上下文：解析文本中 `@token` 得资源 path 列表 + 待发图片。
    // 以文本里实际出现的 @ 为准（用户删掉 @token 即不引用）。
    // @fileName → 文件 path；@skillName → skill://<name> 虚拟 path（后端按 path 查 documents 表）。
    const mounted_paths = resolveMentionedPaths(content, mountedDocs, skills);

    // 手打 @skillName（未走菜单选中即激活）的兑底：发送前对引用但未激活的 skill 补激活。
    // builtin/external 默认进 preamble（isSkillActive=true）无需补；imported/dmi 默认不进，
    // 不补激活则后端 active_skill_doc_paths 不返回其 path，模型 read_document 查不到。
    const mentionedSkills = resolveMentionedSkillNames(content, skills);
    for (const name of mentionedSkills) {
      const s = skills.find((x) => x.name === name);
      if (s && !isSkillActive(s) && !s.globally_disabled) {
        setSkillEnabled.mutate({
          skillName: s.name,
          source: s.source,
          enabled: true,
        });
      }
    }

    const context_images = pendingImages.map((p) => ({
      mime: p.mime,
      data_b64: p.dataB64,
    }));

    clearDraft(conversationId);
    setPendingImages([]);
    await chat.sendAsync({
      content,
      ctx: { mounted_paths, context_images },
      enableReasoning: reasoningEnabled,
    });
    // 自动命名已移至 useChat 的 onSuccess（首条 AI 回复结束后触发 LLM 概括，见 useChat.ts）
  };

  /** ⌘O 文件对话框：文档走 ingest，图片直接读 base64。 */
  const pickFiles = async () => {
    let selected: string | string[] | null = null;
    try {
      selected = await open({
        multiple: true,
        filters: [{ name: "文档与图片", extensions: ALL_EXTENSIONS }],
      });
    } catch (e) {
      console.error("[pickFiles] dialog.open 失败:", e);
      return;
    }
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;

    // 按类型分流
    const docPaths: string[] = [];
    const imgPaths: string[] = [];
    for (const p of paths) {
      const ext = p.split(".").pop()?.toLowerCase() ?? "";
      if (IMAGE_EXTENSIONS.includes(ext)) imgPaths.push(p);
      else docPaths.push(p);
    }

    // 文档 → ingest
    if (docPaths.length > 0) {
      ingest.mutate(
        { paths: docPaths, folderPath: null },
        {
          onError: (e) => console.error("[pickFiles] ingest 失败:", e),
        },
      );
    }

    // 图片 → 降采样后读 base64 入待发（控制 body 体积，防 413）
    if (imgPaths.length > 0) {
      const imgs: PendingImage[] = [];
      const notes: string[] = [];
      for (const p of imgPaths) {
        try {
          const bytes = await readFile(p);
          const fileName = p.split(/[\\/]/).pop() ?? p;
          // 原图超限直接跳过（降采样也救不了异常大的图）
          const sizeErr = validateImageSize(bytes.length, fileName);
          if (sizeErr) {
            notes.push(sizeErr);
            continue;
          }
          const resized = await resizeImageToBase64(bytes);
          if (resized.resized) {
            notes.push(
              `图片「${fileName}」已压缩：${(resized.originalBytes / 1024).toFixed(0)}KB → ${(resized.resizedBytes / 1024).toFixed(0)}KB`,
            );
          }
          // 降采样后仍超限（罕见，如极端高分辨率），跳过
          const postErr = validateImageSize(resized.resizedBytes, fileName);
          if (postErr) {
            notes.push(postErr);
            continue;
          }
          imgs.push({
            id: `img-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
            fileName,
            mime: resized.mime,
            dataB64: resized.dataB64,
          });
        } catch (e) {
          console.error("[pickFiles] 读取图片失败:", p, e);
        }
      }
      if (imgs.length > 0) {
        setPendingImages((prev) => [...prev, ...imgs]);
      }
      if (notes.length > 0) {
        setNotice(notes.join("；"));
      }
    }
  };

  /** 粘贴图片：拦截剪贴板图片，读为 base64 入待发。 */
  const onPaste = async (e: React.ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imageItems = Array.from(items).filter((it) =>
      it.type.startsWith("image/"),
    );
    if (imageItems.length === 0) return;
    e.preventDefault();
    const notes: string[] = [];
    const newImgs: PendingImage[] = [];
    for (const it of imageItems) {
      const file = it.getAsFile();
      if (!file) continue;
      try {
        const buf = await file.arrayBuffer();
        const bytes = new Uint8Array(buf);
        const fileName = file.name || `pasted-${Date.now()}.png`;
        const sizeErr = validateImageSize(bytes.length, fileName);
        if (sizeErr) {
          notes.push(sizeErr);
          continue;
        }
        const resized = await resizeImageToBase64(bytes);
        if (resized.resized) {
          notes.push(
            `图片「${fileName}」已压缩：${(resized.originalBytes / 1024).toFixed(0)}KB → ${(resized.resizedBytes / 1024).toFixed(0)}KB`,
          );
        }
        const postErr = validateImageSize(resized.resizedBytes, fileName);
        if (postErr) {
          notes.push(postErr);
          continue;
        }
        newImgs.push({
          id: `img-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          fileName,
          mime: resized.mime,
          dataB64: resized.dataB64,
        });
      } catch (e) {
        console.error("[onPaste] 处理剪贴图像失败:", e);
      }
    }
    if (newImgs.length > 0) {
      setPendingImages((prev) => [...prev, ...newImgs]);
    }
    if (notes.length > 0) {
      setNotice(notes.join("；"));
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // ⌘↵ / Ctrl↵ 发送
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      send();
      return;
    }
    // 流式中 ⌘. 中断
    if ((e.metaKey || e.ctrlKey) && e.key === ".") {
      e.preventDefault();
      chat.stop();
    }
  };

  const isSending = chat.isSending;
  const isIngesting = ingest.isPending;
  // placeholder 动态带模型名（参考 DeepSeek「给 {模型} 发送消息」）
  const modelName = providerCfg?.model ?? "AI";
  const placeholder = hasProvider
    ? `给 ${modelName} 发送消息…  (⌘↵ 发送，⌘O 添加文件，@ 挂载)`
    : "请先配置模型提供商…";
  const mobilePlaceholder = hasProvider
    ? `给 ${modelName} 发送消息…`
    : "请先配置模型提供商…";
  const isMobile = useIsMobile();

  return (
    <div className="border-t border-border bg-bg p-3 max-md:p-2 max-md:pb-[max(0.5rem,env(safe-area-inset-bottom))]">
      <div className="mx-auto max-w-3xl">
        {!hasProvider && (
          <div className="mb-2 flex items-center gap-2 rounded-md border border-border bg-bg-elevated px-3 py-1.5 text-xs text-fg-muted">
            <span>未配置模型提供商</span>
            <button
              onClick={() => setPaletteOpen(true)}
              className="ml-auto flex items-center gap-1 text-accent hover:underline"
            >
              <Settings size={12} /> 前往设置
            </button>
          </div>
        )}

        {/* 图片处理提示（降采样/超限跳过） */}
        {notice && (
          <div className="mb-2 flex items-center gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 text-xs text-amber-600 dark:text-amber-400">
            <AlertTriangle size={12} className="shrink-0" />
            <span className="min-w-0 flex-1">{notice}</span>
            <button
              onClick={() => setNotice(null)}
              className="shrink-0 rounded p-0.5 hover:bg-amber-500/20"
              title="关闭"
            >
              <X size={11} />
            </button>
          </div>
        )}

        {/* 待发图片 chips（文档挂载在 ScopeChip popover 展示，此处不重复） */}
        {pendingImages.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-1.5">
            {pendingImages.map((p) => {
              const { Icon, className: iconCls } = getFileIcon(
                "image",
                p.fileName,
              );
              return (
                <div
                  key={p.id}
                  className="group flex items-center gap-1 rounded-md border border-accent/50 bg-accent/10 px-2 py-1 text-xs"
                >
                  <Icon size={12} className={`shrink-0 ${iconCls}`} />
                  <span className="max-w-[120px] truncate">{p.fileName}</span>
                  <button
                    onClick={() =>
                      setPendingImages((prev) =>
                        prev.filter((x) => x.id !== p.id),
                      )
                    }
                    className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover hover:text-danger"
                    title="移除"
                  >
                    <X size={11} />
                  </button>
                </div>
              );
            })}
          </div>
        )}

        {/* 输入区：textarea 独占满宽，工具行下移（参考 DeepSeek 布局） */}
        <div className="relative rounded-lg border border-border bg-bg-elevated p-2 focus-within:border-accent">
          <MentionMenu
            conversationId={conversationId}
            textareaRef={textareaRef}
            text={text}
            onTextChange={(next) => setDraft(conversationId, next)}
          />
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setDraft(conversationId, e.target.value)}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            placeholder={isMobile ? mobilePlaceholder : placeholder}
            disabled={!hasProvider}
            rows={1}
            className="max-h-[200px] min-h-[24px] w-full resize-none bg-transparent px-1 py-1 text-sm outline-none placeholder:text-fg-subtle disabled:cursor-not-allowed max-md:text-base"
          />

          {/* 工具行：toggle/动作按钮在左，发送在右 */}
          <div className="mt-1.5 flex items-center gap-1 border-t border-border pt-1.5">
            {/* 添加文件（动作按钮） */}
            <button
              onClick={pickFiles}
              disabled={isIngesting}
              title="添加文件 (⌘O)"
              className="flex shrink-0 items-center rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover hover:text-fg disabled:opacity-40"
            >
              <Paperclip size={16} />
            </button>
            {/* 挂载文件（动作按钮） */}
            <button
              onClick={insertAt}
              title="挂载已摄入文件 (@)"
              className="flex shrink-0 items-center rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover hover:text-fg"
            >
              <AtSign size={16} />
            </button>
            {/* 会话技能开关（方案 C：原 Inspector 第三栏收进 Composer popover） */}
            <SkillPopover conversationId={conversationId} />
            {/* 深度思考：toggle 按钮（文字常显，选中高亮，与动作按钮视觉区分） */}
            <button
              onClick={() =>
                setReasoningEnabled(conversationId, !reasoningEnabled)
              }
              title={reasoningEnabled ? "深度思考已开启" : "开启深度思考"}
              aria-pressed={reasoningEnabled}
              className={`flex shrink-0 items-center gap-1 rounded-md px-2 py-1.5 text-xs transition-colors ${
                reasoningEnabled
                  ? "bg-accent/15 text-accent"
                  : "text-fg-subtle hover:bg-bg-hover hover:text-fg"
              }`}
            >
              <Brain size={14} />
              <span className="max-md:hidden">深度思考</span>
            </button>

            {/* 发送/中断：右端圆形填充主色按钮（视觉权重最高） */}
            <div className="ml-auto">
              {isSending ? (
                <button
                  onClick={chat.stop}
                  title="中断 (⌘.)"
                  className="flex shrink-0 items-center justify-center rounded-full border border-border p-2 text-fg-muted hover:bg-bg-hover"
                >
                  <Square size={14} />
                </button>
              ) : (
                <button
                  onClick={send}
                  disabled={!text.trim() || !hasProvider}
                  title="发送 (⌘↵)"
                  className="flex shrink-0 items-center justify-center rounded-full bg-accent p-2 text-accent-fg hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
                >
                  <ArrowUp size={16} />
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
