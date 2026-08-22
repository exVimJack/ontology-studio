// UI 层：Sidebar.tsx（§20.5）
// 新建会话 + 搜索 + 会话列表（按日分组 + 置顶）。

import { useMemo, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import {
  Plus,
  Search,
  Pin,
  Trash2,
  MessageSquare,
  Database,
  FolderOpen,
  Sparkles,
  Boxes,
} from "lucide-react";
import {
  useConversations,
  useCreateConversation,
  useDeleteConversation,
  useTogglePinned,
} from "@/hooks/useConversations";
import { useCurrentConversationId } from "@/hooks/useCurrentConversationId";
import { DATE_GROUP_ORDER, dateGroup, relativeTime } from "@/lib/domain";
import { useUiStore } from "@/stores/ui-store";
import type { ConversationSummary } from "@/lib/domain";

export function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
  const [query, setQuery] = useState("");
  const { data: conversations = [] } = useConversations();
  const createConv = useCreateConversation();
  const deleteConv = useDeleteConversation();
  const togglePin = useTogglePinned();
  const currentId = useCurrentConversationId();
  const navigate = useNavigate();

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return conversations;
    return conversations.filter(
      (c) =>
        c.title.toLowerCase().includes(q) ||
        (c.last_message_preview ?? "").toLowerCase().includes(q),
    );
  }, [conversations, query]);

  // 置顶组 + 按日分组
  const pinned = filtered.filter((c) => c.pinned);
  const groups = useMemo(() => {
    const map = new Map<string, ConversationSummary[]>();
    for (const c of filtered.filter((c) => !c.pinned)) {
      const g = dateGroup(c.updated_at);
      if (!map.has(g)) map.set(g, []);
      map.get(g)!.push(c);
    }
    return DATE_GROUP_ORDER.filter((g) => map.has(g)).map((g) => ({
      label: g,
      items: map.get(g)!,
    }));
  }, [filtered]);

  return (
    <div className="flex h-full flex-col bg-bg-elevated">
      {/* 顶部：新建 + 搜索 */}
      <div className="flex flex-col gap-2 p-2">
        <button
          onClick={() => createConv.mutate({ title: null })}
          style={{
            backgroundColor: "var(--accent)",
            color: "var(--accent-fg)",
          }}
          className="flex items-center justify-center gap-2 rounded-md px-3 py-2 text-sm font-medium hover:opacity-90"
        >
          <Plus size={16} />
          新建会话
        </button>
        <div className="relative">
          <Search
            size={14}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-subtle"
          />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索会话…"
            className="w-full rounded-md border border-border bg-bg py-1.5 pl-8 pr-2 text-sm outline-none focus:border-accent"
          />
        </div>

        {/* 功能入口（与豆包同层级：知识库 / 数据源 / 技能 / 本体）*/}
        <div className="grid grid-cols-4 gap-1">
          <FeatureEntry
            icon={<FolderOpen size={14} />}
            label="知识库"
            onClick={() => {
              navigate({ to: "/library" });
              onNavigate?.();
            }}
          />
          <FeatureEntry
            icon={<Database size={14} />}
            label="数据源"
            onClick={() => {
              useUiStore.getState().setFederationOpen(true);
              onNavigate?.();
            }}
          />
          <FeatureEntry
            icon={<Sparkles size={14} />}
            label="技能"
            onClick={() => {
              useUiStore.getState().setSkillsOpen(true);
              onNavigate?.();
            }}
          />
          <FeatureEntry
            icon={<Boxes size={14} />}
            label="本体"
            onClick={() => {
              useUiStore.getState().setOntologyOpen(true);
              onNavigate?.();
            }}
          />
        </div>
      </div>

      {/* 列表 */}
      <div className="flex-1 overflow-y-auto px-1 pb-2">
        {pinned.length > 0 && (
          <Group label="置顶">
            {pinned.map((c) => (
              <ConversationItem
                key={c.id}
                conv={c}
                active={c.id === currentId}
                onClick={() => {
                  navigate({
                    to: "/chat/$conversationId",
                    params: { conversationId: c.id },
                  });
                  onNavigate?.();
                }}
                onPin={() => togglePin.mutate({ id: c.id, pinned: !c.pinned })}
                onDelete={() => deleteConv.mutate(c.id)}
              />
            ))}
          </Group>
        )}
        {groups.map((g) => (
          <Group key={g.label} label={g.label}>
            {g.items.map((c) => (
              <ConversationItem
                key={c.id}
                conv={c}
                active={c.id === currentId}
                onClick={() => {
                  navigate({
                    to: "/chat/$conversationId",
                    params: { conversationId: c.id },
                  });
                  onNavigate?.();
                }}
                onPin={() => togglePin.mutate({ id: c.id, pinned: !c.pinned })}
                onDelete={() => deleteConv.mutate(c.id)}
              />
            ))}
          </Group>
        ))}
        {filtered.length === 0 && (
          <div className="px-3 py-8 text-center text-xs text-fg-subtle">
            {query ? "无匹配会话" : "暂无会话，点击上方新建"}
          </div>
        )}
      </div>
    </div>
  );
}

function FeatureEntry({
  icon,
  label,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center justify-center gap-1.5 rounded-md border border-border bg-bg px-1 py-1.5 text-[11px] text-fg hover:bg-bg-hover"
    >
      {icon}
      {label}
    </button>
  );
}

function Group({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mb-1">
      <div className="px-3 py-1 text-[11px] font-medium uppercase tracking-wide text-fg-subtle">
        {label}
      </div>
      {children}
    </div>
  );
}

function ConversationItem({
  conv,
  active,
  onClick,
  onPin,
  onDelete,
}: {
  conv: ConversationSummary;
  active: boolean;
  onClick: () => void;
  onPin: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      onClick={onClick}
      className={`group mx-1 flex cursor-pointer items-start gap-2 rounded-md px-2 py-2 ${
        active ? "bg-bg-hover" : "hover:bg-bg-hover/50"
      }`}
    >
      <MessageSquare size={14} className="mt-0.5 shrink-0 text-fg-subtle" />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-1">
          <span className={`truncate text-sm ${active ? "font-medium" : ""}`}>
            {conv.title || "新会话"}
          </span>
          <span className="shrink-0 text-[10px] text-fg-subtle">
            {relativeTime(conv.updated_at)}
          </span>
        </div>
        {conv.last_message_preview && (
          <div className="truncate text-xs text-fg-muted">
            {conv.last_message_preview}
          </div>
        )}
      </div>
      <div className="flex shrink-0 gap-0.5 opacity-0 group-hover:opacity-100">
        <button
          onClick={(e) => {
            e.stopPropagation();
            onPin();
          }}
          className={`rounded p-1 hover:bg-bg ${conv.pinned ? "text-accent" : "text-fg-subtle"}`}
          title={conv.pinned ? "取消置顶" : "置顶"}
        >
          <Pin size={12} />
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          className="rounded p-1 text-fg-subtle hover:bg-bg hover:text-danger"
          title="删除"
        >
          <Trash2 size={12} />
        </button>
      </div>
    </div>
  );
}
