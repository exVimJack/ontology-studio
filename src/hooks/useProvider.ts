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
    },
  })
}

/** 是否已配置 provider（决定 Composer 是否可发送）。 */
export function useHasProvider(): boolean {
  const { data } = useProvider()
  return !!data && !!data.api_key && !!data.model
}
