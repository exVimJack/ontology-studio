"use client";

import "@assistant-ui/react-markdown/styles/dot.css";

import {
  type CodeHeaderProps,
  MarkdownTextPrimitive,
  unstable_memoizeMarkdownComponents as memoizeMarkdownComponents,
  useIsMarkdownCodeBlock,
} from "@assistant-ui/react-markdown";
import remarkGfm from "remark-gfm";
import { type FC, memo, useState } from "react";
import { CheckIcon, CopyIcon } from "lucide-react";
import { remarkCitations } from "./remarkCitations";

/**
 * assistant 消息的 markdown 渲染组件。
 *
 * 采用官方 markdown-text 组件方案（@assistant-ui/react-markdown），
 * 而非 streamdown——理由：
 *  - 自带 aui-md 排版（紧凑间距，经过官方调优），无需手写 CSS
 *  - dot.css 提供流式脉动光标
 *  - 内置代码块复制按钮 + 语言标签
 *  - 基于 react-markdown，生态稳定
 *
 * components 全部在模块作用域定义（memoizeMarkdownComponents 缓存），
 * 避免流式时每次 render 新建引用导致子树重渲染。
 */

const MarkdownTextImpl = () => {
  return (
    <MarkdownTextPrimitive
      remarkPlugins={[remarkGfm, remarkCitations]}
      className="aui-md"
      components={defaultComponents}
      defer
    />
  );
};

export const MarkdownText = memo(MarkdownTextImpl);

// ───────── 代码块头部：语言标签 + 复制按钮 ─────────

const CodeHeader: FC<CodeHeaderProps> = ({ language, code }) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const onCopy = () => {
    if (!code || isCopied) return;
    copyToClipboard(code);
  };

  return (
    <div className="mt-3 flex items-center justify-between rounded-t-xl border border-b-0 border-border/50 bg-bg-hover/50 px-3.5 py-1.5 text-xs">
      <span className="font-medium lowercase text-fg-muted">{language}</span>
      <button
        onClick={onCopy}
        className="rounded p-1 text-fg-muted hover:bg-bg-hover hover:text-fg"
        title="复制"
      >
        {!isCopied && <CopyIcon size={13} />}
        {isCopied && <CheckIcon size={13} className="text-accent" />}
      </button>
    </div>
  );
};

const useCopyToClipboard = ({
  copiedDuration = 3000,
}: {
  copiedDuration?: number;
} = {}) => {
  const [isCopied, setIsCopied] = useState<boolean>(false);

  const copyToClipboard = (value: string) => {
    if (!value || typeof navigator === "undefined" || !navigator.clipboard) {
      return;
    }

    navigator.clipboard.writeText(value).then(
      () => {
        setIsCopied(true);
        setTimeout(() => setIsCopied(false), copiedDuration);
      },
      () => {},
    );
  };

  return { isCopied, copyToClipboard };
};

// ───────── Citation 角标 ─────────
// 正文里的 [n] 转为上标小数字，点击/悬停提示去右侧来源面板查看。
// 不在此处显示来源卡片（拿不到 message 上下文），来源详情统一在 ScopeChip popover 的挂载文件区。
const CitationMark: FC<{ index: number }> = ({ index }) => (
  <sup
    className="ms-0.5 cursor-help rounded bg-accent/15 px-1 text-[0.7em] font-medium text-accent"
    title={`挂载文档 ${index}（详见右侧「挂载文档」面板）`}
  >
    {index}
  </sup>
);

// ───────── markdown 元素 → 带紧凑排版的 className ─────────
// 直接照搬官方 markdown-text 的间距值（mt-5 mb-2 / my-3 等）。

const defaultComponents = memoizeMarkdownComponents({
  h1: ({ className, ...props }) => (
    <h1
      className="aui-md-h1 mt-5 mb-2 scroll-m-20 text-xl font-semibold first:mt-0 last:mb-0"
      {...props}
    />
  ),
  h2: ({ className, ...props }) => (
    <h2
      className="aui-md-h2 mt-5 mb-2 scroll-m-20 text-lg font-semibold first:mt-0 last:mb-0"
      {...props}
    />
  ),
  h3: ({ className, ...props }) => (
    <h3
      className="aui-md-h3 mt-4 mb-1.5 scroll-m-20 text-base font-semibold first:mt-0 last:mb-0"
      {...props}
    />
  ),
  h4: ({ className, ...props }) => (
    <h4
      className="aui-md-h4 mt-3.5 mb-1 scroll-m-20 text-base font-medium first:mt-0 last:mb-0"
      {...props}
    />
  ),
  h5: ({ className, ...props }) => (
    <h5
      className="aui-md-h5 mt-3 mb-1 text-sm font-semibold first:mt-0 last:mb-0"
      {...props}
    />
  ),
  h6: ({ className, ...props }) => (
    <h6
      className="aui-md-h6 mt-3 mb-1 text-sm font-medium first:mt-0 last:mb-0"
      {...props}
    />
  ),
  p: ({ className, ...props }) => (
    <p
      className="aui-md-p my-3 leading-relaxed first:mt-0 last:mb-0 [overflow-wrap:anywhere]"
      {...props}
    />
  ),
  a: ({ className, ...props }) => {
    const href = props.href ?? ""
    // Citation 角标：href 为 #cite-n，渲染为上标数字
    const citeMatch = href.match(/^#cite-(\d+)$/)
    if (citeMatch) {
      return <CitationMark index={Number(citeMatch[1])} />
    }
    return (
      <a
        className="text-accent underline underline-offset-2 hover:opacity-80"
        {...props}
      />
    )
  },
  blockquote: ({ className, ...props }) => (
    <blockquote
      className="my-3 border-s-2 border-border ps-4 text-fg-muted"
      {...props}
    />
  ),
  ul: ({ className, ...props }) => (
    <ul
      className="my-3 ms-5 list-disc marker:text-fg-subtle [&>li]:mt-1"
      {...props}
    />
  ),
  ol: ({ className, ...props }) => (
    <ol
      className="my-3 ms-5 list-decimal marker:text-fg-subtle [&>li]:mt-1"
      {...props}
    />
  ),
  hr: ({ className, ...props }) => (
    <hr className="my-3 border-border" {...props} />
  ),
  table: ({ className, ...props }) => (
    <div className="my-3 overflow-x-auto">
      <table
        className="w-full border-separate border-spacing-0 text-sm"
        {...props}
      />
    </div>
  ),
  th: ({ className, ...props }) => (
    <th
      className="bg-bg-hover px-3 py-1.5 text-start font-medium first:rounded-ss-lg last:rounded-se-lg [[align=center]]:text-center [[align=right]]:text-right"
      {...props}
    />
  ),
  td: ({ className, ...props }) => (
    <td
      className="border-b border-s border-border/60 px-3 py-1.5 text-start last:border-e [[align=center]]:text-center [[align=right]]:text-right"
      {...props}
    />
  ),
  tr: ({ className, ...props }) => (
    <tr
      className="m-0 border-b border-border/60 p-0 first:border-t [&:last-child>td:first-child]:rounded-es-lg [&:last-child>td:last-child]:rounded-ee-lg"
      {...props}
    />
  ),
  li: ({ className, ...props }) => (
    <li className="leading-relaxed [overflow-wrap:anywhere]" {...props} />
  ),
  strong: ({ className, ...props }) => (
    <strong className="font-semibold" {...props} />
  ),
  sup: ({ className, ...props }) => (
    <sup className="[&>a]:text-xs [&>a]:no-underline" {...props} />
  ),
  pre: ({ className, ...props }) => (
    <pre
      className="overflow-x-auto rounded-b-xl border border-t-0 border-border/50 bg-bg-hover/30 p-3.5 text-[13px] leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]"
      {...props}
    />
  ),
  code: function Code({ className, ...props }) {
    const isCodeBlock = useIsMarkdownCodeBlock();
    return (
      <code
        className={
          !isCodeBlock
            ? "rounded-md bg-bg-hover px-1.5 py-0.5 font-mono text-[0.85em] [overflow-wrap:anywhere]"
            : "whitespace-pre-wrap [overflow-wrap:anywhere]"
        }
        {...props}
      />
    );
  },
  CodeHeader,
});
