// UI 层：CommandPalette.tsx（§20.5 ⌘K）
// 命令面板：导航 + 新建会话 + 打开设置 + 切换主题。
// 用 cmdk。

import { Command } from "cmdk";
import { useEffect } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useUiStore } from "@/stores/ui-store";
import {
  useCreateConversation,
  useConversations,
} from "@/hooks/useConversations";
import { useCurrentConversationId } from "@/hooks/useCurrentConversationId";
import { applyTheme } from "@/stores/ui-store";
import {
  Plus,
  Settings,
  Sun,
  Moon,
  MessageSquare,
  Search,
  FolderOpen,
  Sparkles,
  Boxes,
} from "lucide-react";

export function CommandPalette() {
  const open = useUiStore((s) => s.commandPaletteOpen);
  const setOpen = useUiStore((s) => s.setCommandPaletteOpen);
  const setTheme = useUiStore((s) => s.setTheme);
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen);
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
                <Kbd>⌘N</Kbd>
              </Item>
              <Item
                onSelect={run(() => setSettingsOpen(true))}
                icon={<Settings size={14} />}
              >
                打开设置
                <Kbd>⌘,</Kbd>
              </Item>
              <Item
                onSelect={run(() => navigate({ to: "/library" }))}
                icon={<FolderOpen size={14} />}
              >
                文件库
              </Item>
              <Item
                onSelect={run(() => useUiStore.getState().setSkillsOpen(true))}
                icon={<Sparkles size={14} />}
              >
                技能管理
              </Item>
              <Item
                onSelect={run(() =>
                  useUiStore.getState().setOntologyOpen(true),
                )}
                icon={<Boxes size={14} />}
              >
                本体
                <Kbd>⌘⇧O</Kbd>
              </Item>
              <Item
                onSelect={run(() => setTheme("light"))}
                icon={<Sun size={14} />}
              >
                浅色主题
              </Item>
              <Item
                onSelect={run(() => setTheme("dark"))}
                icon={<Moon size={14} />}
              >
                深色主题
              </Item>
              <Item
                onSelect={run(() => {
                  setTheme("system");
                  applyTheme("system");
                })}
                icon={<Sun size={14} />}
              >
                跟随系统主题
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
