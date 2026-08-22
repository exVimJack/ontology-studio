// UI 层：ChatEmptyState.tsx
// 无选中会话时的引导态。

import { Plus } from "lucide-react"
import { useCreateConversation } from "@/hooks/useConversations"
import { useProvider } from "@/hooks/useProvider"
import { PROVIDER_PRESETS } from "@/lib/domain"

export function ChatEmptyState() {
  const createConv = useCreateConversation()
  const { data: provider } = useProvider()

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 p-8 text-center">
      <div className="text-5xl">💬</div>
      <h2 className="text-xl font-semibold">开始一段新对话</h2>
      <p className="max-w-md text-sm text-fg-muted">
        {provider
          ? "选择左侧会话，或新建一个开始与 AI 对话。"
          : "首次使用请先在设置中配置模型提供商。"}
      </p>

      {!provider && (
        <div className="rounded-lg border border-border bg-bg-elevated p-4 text-left text-sm">
          <div className="mb-2 font-medium">推荐配置：</div>
          <ul className="space-y-1 text-fg-muted">
            {PROVIDER_PRESETS.slice(0, 4).map((p) => (
              <li key={p.label}>
                <span className="text-fg">{p.label}</span> — {p.models[0] ?? "自定义模型"}
              </li>
            ))}
          </ul>
          <div className="mt-2 text-xs text-fg-subtle">按 ⌘K 打开命令面板 → 设置</div>
        </div>
      )}

      <button
        onClick={() => createConv.mutate({ title: null })}
        className="flex items-center gap-2 rounded-md bg-accent px-4 py-2 text-sm font-medium text-accent-fg hover:opacity-90"
      >
        <Plus size={16} />
        新建会话
      </button>
    </div>
  )
}
