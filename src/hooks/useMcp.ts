// Hooks 层：useMcp.ts（二期 A3）
// MCP server 配置与工具的 TanStack Query 封装。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type { McpServerConfig, McpServerStatus, McpToolDef } from "@/lib/domain"

const QK_MCP_SERVERS = ["mcp-servers"] as const
const QK_MCP_TOOLS = ["mcp-tools"] as const

/** 已持久化的 MCP server 配置列表。 */
export function useMcpServers() {
  return useQuery<McpServerConfig[]>({
    queryKey: QK_MCP_SERVERS,
    queryFn: () => ipc.getMcpServers(),
  })
}

/** 当前已注册的 MCP 工具（已连接 server 暴露的）。 */
export function useMcpTools() {
  return useQuery<McpToolDef[]>({
    queryKey: QK_MCP_TOOLS,
    queryFn: () => ipc.listMcpTools(),
  })
}

/** 配置并连接 MCP server（整体替换）。返回每个 server 的连接状态。 */
export function useSetMcpServers() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (servers: McpServerConfig[]) => ipc.setMcpServers(servers),
    onSuccess: (_statuses: McpServerStatus[]) => {
      // 配置变更后刷新工具列表与配置缓存
      qc.invalidateQueries({ queryKey: QK_MCP_SERVERS })
      qc.invalidateQueries({ queryKey: QK_MCP_TOOLS })
    },
  })
}
