// Hooks 层：useProvider.ts（§12.2 / §20.9）
// Provider 配置的 TanStack Query 封装。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type { ProviderConfig, SetProviderInput } from "@/lib/domain"

const QK_PROVIDER = ["provider"] as const

export function useProvider() {
  return useQuery<ProviderConfig | null>({
    queryKey: QK_PROVIDER,
    queryFn: () => ipc.getProvider(),
  })
}

export function useSetProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: SetProviderInput) => ipc.setProvider(input),
    onSuccess: (cfg) => {
      qc.setQueryData(QK_PROVIDER, cfg)
      // 窗口随 provider 变（五层 fallback 重新解析）→ 用量条基准刷新
      qc.invalidateQueries({ queryKey: ["context-window"] })
    },
  })
}

/** 是否已配置 provider（决定 Composer 是否可发送）。 */
export function useHasProvider(): boolean {
  const { data } = useProvider()
  return !!data && !!data.api_key && !!data.model
}

// ── 校验者 B 独立 provider 配置（§4.4 独立性，与建模者 A 完全对等）──

const QK_VERIFIER = ["verifier-provider"] as const

export function useVerifierProvider() {
  return useQuery({
    queryKey: QK_VERIFIER,
    queryFn: () => ipc.getVerifierProvider(),
  })
}

export function useSetVerifierProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: SetProviderInput) => ipc.setVerifierProvider(input),
    onSuccess: (cfg) => qc.setQueryData(QK_VERIFIER, cfg),
  })
}
