// UI 层：CommandPalette.tsx（§20.5 ⌘K）
// 命令面板：导航 + 新建会话 + 模型/MCP 设置快捷入口 + 会话切换。
// 用 cmdk。

import { shortcut } from "@/lib/keyboard";

import { Command } from "cmdk";
import { useEffect } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useUiStore } from "@/stores/ui-store";
import {
  useCreateConversation,
  useConversations,
} from "@/hooks/useConversations";
import { useCurrentConversationId } from "@/hooks/useCurrentConversationId";
import {
  Plus,
  Server,
  MessageSquare,
  Search,
  FolderOpen,
  Sparkles,
  Boxes,
  Cpu,
} from "lucide-react";

export function CommandPalette() {
  const open = useUiStore((s) => s.commandPaletteOpen);
  const setOpen = useUiStore((s) => s.setCommandPaletteOpen);
  const { data: conversations = [] } = useConversations();
  const createConv = useCreateConversation();
  const currentId = useCurrentConversationId();
  const navigate = useNavigate();

  // ESC 关闭由 cmdk 内部处理；这里额外兜底
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    if (open) window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, setOpen]);

  if (!open) return null;

  const run = (fn: () => void) => () => {
    fn();
    setOpen(false);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/30 pt-[15vh] max-md:pt-0 max-md:items-start"
      onClick={() => setOpen(false)}
    >
      <div
        className="w-full max-w-xl rounded-lg border border-border bg-bg shadow-2xl max-md:max-w-none max-md:rounded-none max-md:border-x-0 max-md:border-t-0 max-md:pt-[env(safe-area-inset-top)]"
        onClick={(e) => e.stopPropagation()}
      >
        <Command label="命令面板">
          <div className="flex items-center gap-2 border-b border-border px-3">
            <Search size={15} className="text-fg-subtle" />
            <Command.Input
              autoFocus
              placeholder="输入命令或搜索会话…"
              className="w-full bg-transparent py-3 text-sm outline-none placeholder:text-fg-subtle"
            />
          </div>
          <Command.List className="max-h-[50vh] overflow-y-auto p-1 max-md:max-h-[70vh]">
            <Command.Empty className="px-3 py-6 text-center text-sm text-fg-subtle">
              无匹配结果
            </Command.Empty>

            <Command.Group heading="操作" className="text-xs text-fg-subtle">
              <Item
                onSelect={run(() => createConv.mutate({ title: null }))}
                icon={<Plus size={14} />}
              >
                新建会话
                <Kbd>{shortcut(["mod", "N"])}</Kbd>
              </Item>
              <Item
                onSelect={run(() => useUiStore.getState().setModelSettingsOpen(true))}
                icon={<Cpu size={14} />}
              >
                模型设置
                <Kbd>{shortcut(["mod", "M"])}</Kbd>
              </Item>
              <Item
                onSelect={run(() => useUiStore.getState().setMcpSettingsOpen(true))}
                icon={<Server size={14} />}
              >
                工具服务器 (MCP)
                <Kbd>{shortcut(["mod", "shift", "M"])}</Kbd>
              </Item>
              <Item
                onSelect={run(() => {
                  useUiStore.getState().setActiveNav("library");
                  navigate({ to: "/library" });
                })}
                icon={<FolderOpen size={14} />}
              >
                文件库
              </Item>
              <Item
                onSelect={run(() => useUiStore.getState().setActiveNav("skills"))}
                icon={<Sparkles size={14} />}
              >
                技能管理
              </Item>
              <Item
                onSelect={run(() =>
                  useUiStore.getState().setActiveNav("ontology"),
                )}
                icon={<Boxes size={14} />}
              >
                本体
                <Kbd>{shortcut(["mod", "shift", "O"])}</Kbd>
              </Item>
            </Command.Group>

            {conversations.length > 0 && (
              <Command.Group heading="会话" className="text-xs text-fg-subtle">
                {conversations.slice(0, 20).map((c) => (
                  <Item
                    key={c.id}
                    onSelect={run(() =>
                      navigate({
                        to: "/chat/$conversationId",
                        params: { conversationId: c.id },
                      }),
                    )}
                    icon={<MessageSquare size={14} />}
                  >
                    {c.title || "新会话"}
                    {c.id === currentId && <Kbd>当前</Kbd>}
                  </Item>
                ))}
              </Command.Group>
            )}
          </Command.List>
        </Command>
      </div>
    </div>
  );
}

function Item({
  children,
  icon,
  onSelect,
}: {
  children: React.ReactNode;
  icon: React.ReactNode;
  onSelect: () => void;
}) {
  return (
    <Command.Item
      onSelect={onSelect}
      className="flex cursor-pointer items-center gap-2 rounded-md px-2.5 py-2 text-sm aria-selected:bg-bg-hover"
    >
      <span className="text-fg-subtle">{icon}</span>
      <span className="flex-1">{children}</span>
    </Command.Item>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <span className="ml-auto rounded border border-border bg-bg px-1.5 py-0.5 text-[10px] text-fg-subtle">
      {children}
    </span>
  );
}
