// Hooks 层：useFederation.ts（三期：联邦查询）
// 数据源管理 + schema 浏览 + SQL 执行的 TanStack Query 封装。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type {
  DataSourceConfig,
  DataSourceSummary,
  QueryResult,
  SchemaSnapshot,
  TableMeta,
} from "@/lib/domain"

const QK_SOURCES = ["federation-sources"] as const
const QK_SCHEMA = (catalog: string) => ["federation-schema", catalog] as const
const QK_TABLE = (catalog: string, table: string) =>
  ["federation-table", catalog, table] as const

/** 已注册数据源列表（含连接状态/表数）。 */
export function useDataSources() {
  return useQuery<DataSourceSummary[]>({
    queryKey: QK_SOURCES,
    queryFn: () => ipc.listDataSources(),
  })
}

/** 浏览某 catalog 下所有表结构。 */
export function useFederationSchema(catalog: string | null) {
  return useQuery<SchemaSnapshot>({
    queryKey: QK_SCHEMA(catalog ?? ""),
    queryFn: () => ipc.browseFederationSchema(catalog!),
    enabled: !!catalog,
  })
}

/** 裸查询函数（供 useQueries 批量场景用，避免组件直接 import ipc）。 */
export function fetchFederationSchema(catalog: string) {
  return ipc.browseFederationSchema(catalog)
}

/** 描述单表（列/类型/样本/行数）。 */
export function useTableMeta(catalog: string | null, table: string | null) {
  return useQuery<TableMeta>({
    queryKey: QK_TABLE(catalog ?? "", table ?? ""),
    queryFn: () => ipc.describeFederationTable(catalog!, table!),
    enabled: !!catalog && !!table,
  })
}

/** 注册数据源（落库 + 热注册）。 */
export function useRegisterDataSource() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (config: DataSourceConfig) => ipc.registerDataSource(config),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK_SOURCES }),
  })
}

/** 测试连接（不落库）。 */
export function useTestDataSource() {
  return useMutation({
    mutationFn: (config: DataSourceConfig) => ipc.testDataSource(config),
  })
}

/** 注销数据源。 */
export function useDeregisterDataSource() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => ipc.deregisterDataSource(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK_SOURCES }),
  })
}

/** 执行只读 SQL。 */
export function useExecuteQuery() {
  return useMutation({
    mutationFn: (vars: { sql: string; limit?: number }) =>
      ipc.executeFederationQuery(vars.sql, vars.limit),
  })
}

/** EXPLAIN。 */
export function useExplainQuery() {
  return useMutation({
    mutationFn: (sql: string) => ipc.explainFederationQuery(sql),
  })
}

export type { QueryResult }
