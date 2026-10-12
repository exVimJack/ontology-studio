// UI 层：McpSettingsPanel.tsx（设置重构 ③）
// MCP 工具服务器配置独立弹窗：从 SettingsView 拆出（原 McpSection 整块迁移）。
// 与 ModelSettingsModal 同形态（居中弹窗、遮罩关闭、内部滚动）。
// 入口：NavRail 左下角设置菜单 / ⌘⇧M 快捷键。
// SettingsView（全屏总设置）随之退役：模型/外观/MCP 三块均已有独立入口。

import { useEffect, useState } from "react"
import { useMcpServers, useSetMcpServers, useMcpTools } from "@/hooks/useMcp"
import type { McpServerConfig } from "@/lib/domain"
import { Check, Loader2, Plus, Trash2, Server, Wrench, X } from "lucide-react"

/** 居中弹窗外壳：遮罩点击关闭，内部滚动。 */
export function McpSettingsModal({ onClose }: { onClose: () => void }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 max-md:p-2">
      <div className="absolute inset-0 bg-black/40" onClick={onClose} />
      <div className="relative flex max-h-[85vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-border bg-bg shadow-2xl max-md:max-h-[92vh]">
        <div className="flex shrink-0 items-center justify-between border-b border-border px-5 py-3">
          <h2 className="text-base font-semibold">工具服务器 (MCP)</h2>
          <button
            onClick={onClose}
            className="rounded p-1.5 text-fg-subtle hover:bg-bg-hover hover:text-fg"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>
        <div className="overflow-y-auto px-5 py-4">
          <McpSettingsPanel />
        </div>
      </div>
    </div>
  )
}

/** MCP 工具服务器配置体。 */
export function McpSettingsPanel() {
  const { data: savedServers = [] } = useMcpServers()
  const { data: tools = [] } = useMcpTools()
  const setMcp = useSetMcpServers()
  // 本地编辑态：从已存配置初始化，编辑后点保存才连接
  const [servers, setServers] = useState<McpServerConfig[]>([])
  const [statuses, setStatuses] = useState<{ name: string; connected: boolean; tool_count: number; error?: string | null }[]>([])

  useEffect(() => {
    if (savedServers.length && servers.length === 0) {
      setServers(savedServers)
    }
  }, [savedServers])

  const addStdio = () => {
    setServers([
      ...servers,
      { kind: "stdio", id: crypto.randomUUID(), name: "", command: "", args: [], env: {} },
    ])
  }
  const addHttp = () => {
    setServers([
      ...servers,
      { kind: "http", id: crypto.randomUUID(), name: "", url: "", auth_token: null, headers: {} },
    ])
  }
  const remove = (id: string) => setServers(servers.filter((s) => s.id !== id))
  const update = (id: string, updater: (s: McpServerConfig) => McpServerConfig) =>
    setServers(servers.map((s) => (s.id === id ? updater(s) : s)))

  const save = async () => {
    const res = await setMcp.mutateAsync(servers)
    setStatuses(res)
  }

  return (
    <div>
      <p className="mb-4 text-xs text-fg-muted">
        接入外部 MCP server，工具自动注入对话。stdio 本地进程 / HTTP 远程服务。
      </p>

      {servers.length === 0 && (
        <p className="text-xs text-fg-subtle">尚未配置 MCP server。点击下方按钮添加。</p>
      )}

      {servers.map((s) => (
        <div key={s.id} className="mb-2 rounded-md border border-border p-3 space-y-2">
          <div className="flex items-center gap-2">
            <Server size={13} className="text-fg-subtle" />
            <span className="text-xs font-medium text-fg-muted">{s.kind === "stdio" ? "stdio" : "http"}</span>
            {statuses.find((st) => st.name === s.name)?.connected === true && (
              <span className="text-[10px] text-emerald-500">已连接</span>
            )}
            {statuses.find((st) => st.name === s.name)?.connected === false && (
              <span className="text-[10px] text-rose-500" title={statuses.find((st) => st.name === s.name)?.error ?? undefined}>
                连接失败
              </span>
            )}
            <button onClick={() => remove(s.id)} className="ml-auto text-fg-subtle hover:text-rose-500">
              <Trash2 size={13} />
            </button>
          </div>
          <input
            value={s.name}
            onChange={(e) => update(s.id, (sv) => ({ ...sv, name: e.target.value }))}
            placeholder="展示名（如 filesystem）"
            className="mcp-input"
          />
          {s.kind === "stdio" ? (
            <>
              <input
                value={s.command}
                onChange={(e) => update(s.id, (sv) => ({ ...sv, command: e.target.value }))}
                placeholder="命令（如 npx）"
                className="mcp-input"
              />
              <input
                value={s.args.join(" ")}
                onChange={(e) =>
                  update(s.id, (sv) => ({ ...sv, args: e.target.value.split(/\s+/).filter(Boolean) }))
                }
                placeholder="参数（空格分隔，如 -y @modelcontextprotocol/server-filesystem /tmp）"
                className="mcp-input"
              />
            </>
          ) : (
            <>
              <input
                value={s.url}
                onChange={(e) => update(s.id, (sv) => ({ ...sv, url: e.target.value }))}
                placeholder="URL（如 https://example.com/mcp）"
                className="mcp-input"
              />
              <input
                value={s.auth_token ?? ""}
                onChange={(e) =>
                  update(s.id, (sv) => ({ ...sv, auth_token: e.target.value || null }))
                }
                placeholder="Bearer token（可选）"
                className="mcp-input"
              />
            </>
          )}
        </div>
      ))}

      <div className="flex gap-2">
        <button onClick={addStdio} className="flex items-center gap-1 rounded-md border border-border px-2.5 py-1.5 text-xs hover:bg-bg-hover">
          <Plus size={12} /> 添加 stdio
        </button>
        <button onClick={addHttp} className="flex items-center gap-1 rounded-md border border-border px-2.5 py-1.5 text-xs hover:bg-bg-hover">
          <Plus size={12} /> 添加 HTTP
        </button>
      </div>

      <div className="mt-4 flex items-center gap-3">
        <button
          onClick={save}
          disabled={setMcp.isPending}
          className="flex items-center gap-1.5 rounded-md bg-accent px-4 py-2 text-sm font-medium text-accent-fg hover:opacity-90 disabled:opacity-40"
        >
          {setMcp.isPending ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
          连接
        </button>
      </div>

      {/* 已注册工具列表 */}
      {tools.length > 0 && (
        <div className="mt-4">
          <div className="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-fg-muted">
            <Wrench size={12} /> 可用工具（{tools.length}）
          </div>
          <div className="space-y-1">
            {tools.map((t) => (
              <div key={t.name} className="rounded border border-border bg-bg-elevated px-2.5 py-1.5">
                <div className="font-mono text-xs font-medium text-fg">{t.name}</div>
                {t.description && <div className="text-[11px] text-fg-subtle">{t.description}</div>}
              </div>
            ))}
          </div>
        </div>
      )}

      <style>{`
        .mcp-input {
          width: 100%;
          border-radius: 6px;
          border: 1px solid var(--border);
          background: var(--bg-elevated);
          padding: 0.5rem 0.75rem;
          font-size: 0.875rem;
          outline: none;
        }
        .mcp-input:focus { border-color: var(--accent); }
      `}</style>
    </div>
  )
}
