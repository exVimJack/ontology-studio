// IPC 层：commands.ts
// 对 tauri-specta 生成的 bindings.ts 做命令式封装，统一 unwrap 结果联合。
// （bindings.ts 的 commands.* 返回 {status:"ok"|"error"} 联合，这里拆开。）
//
// 约束（§12.1）：本文件不 import stores/components；只依赖 bindings + domain。

import { commands as raw } from "@/lib/ipc/bindings";
import type {
  AppError,
  ChatStreamChunk,
  ConversationRow,
  ConversationSummary,
  CreateConversationInput,
  DataSourceConfig,
  DataSourceSummary,
  DeleteMessageInput,
  IngestProgress,
  IngestResultItem,
  McpServerConfig,
  McpServerStatus,
  McpToolDef,
  DocumentSummaryDto,
  DocumentContentDto,
  MessageRow,
  MountedDocDto,
  ProviderConfig,
  QueryResult,
  SchemaSnapshot,
  SendMessageInput,
  SetMessageStatusInput,
  SetPinnedInput,
  SetProviderInput,
  SkillDto,
  SkillSource,
  TableMeta,
  OntologySummary,
  OntologyChangelog,
  OntologyCharter,
} from "@/lib/ipc/bindings";
import type {
  ActiveScopeDto,
  FolderNodeDto,
  OntologyPayload,
} from "@/lib/domain";
import { Channel } from "@tauri-apps/api/core";

/** IPC 调用失败时抛出的错误（携带 AppError 详情）。 */
export class IpcError extends Error {
  readonly appError: AppError;
  constructor(appError: AppError) {
    const msg = "message" in appError ? appError.message : appError.kind;
    super(msg);
    this.name = "IpcError";
    this.appError = appError;
  }
}

/** unwrap {status:"ok"|"error"} 联合，error 抛 IpcError。 */
async function unwrap<T>(
  p: Promise<{ status: "ok"; data: T } | { status: "error"; error: AppError }>,
): Promise<T> {
  const r = await p;
  if (r.status === "ok") return r.data;
  throw new IpcError(r.error);
}

/** 是否用户主动中断（前端静默处理，§15）。 */
export function isCancelled(e: unknown): boolean {
  return e instanceof IpcError && e.appError.kind === "Cancelled";
}

export const ipc = {
  // ── 会话 ──
  createConversation: (input: CreateConversationInput) =>
    unwrap<ConversationRow>(raw.createConversation(input as never)),
  listConversations: () =>
    unwrap<ConversationSummary[]>(raw.listConversations() as never),
  renameConversation: (id: string, title: string) =>
    unwrap<ConversationRow>(raw.renameConversation(id, title)),
  generateConversationTitle: (id: string) =>
    unwrap<ConversationRow>(raw.generateConversationTitle(id)),
  setConversationPinned: (input: SetPinnedInput) =>
    unwrap<null>(raw.setConversationPinned(input)),
  deleteConversation: (id: string) => unwrap<null>(raw.deleteConversation(id)),

  // ── 消息 ──
  listMessages: (conversationId: string, limit?: number | null) =>
    unwrap<MessageRow[]>(
      raw.listMessages(conversationId, limit ?? null) as never,
    ),
  deleteMessage: (input: DeleteMessageInput) =>
    unwrap<null>(raw.deleteMessage(input)),
  /** 删除指定消息及其之后的全部消息（重新生成/编辑重发用，返回删除条数）。 */
  deleteMessageAndAfter: (messageId: string) =>
    unwrap<number>(raw.deleteMessageAndAfter({ message_id: messageId })),
  setMessageStatus: (input: SetMessageStatusInput) =>
    unwrap<null>(raw.setMessageStatus(input as never)),
  /** 中断指定 assistant 消息的流式生成（后端 oneshot 取消信号）。 */
  cancelStream: (input: { messageId: string }) =>
    unwrap<null>(raw.cancelStream(input.messageId)),

  // ── 对话流式 ──
  /**
   * 发送消息并流式接收。返回最终 assistant 消息 ID。
   * onChunk 在每个 chunk 到达时回调（已在主线程，可直接 setState）。
   */
  sendMessage: (
    input: SendMessageInput,
    onChunk: (c: ChatStreamChunk) => void,
  ) => {
    const channel = new Channel<ChatStreamChunk>();
    channel.onmessage = onChunk;
    return unwrap<string>(raw.sendMessage(input, channel));
  },

  // ── provider ──
  setProvider: (input: SetProviderInput) =>
    unwrap<ProviderConfig>(raw.setProvider(input as never)),
  getProvider: () => unwrap<ProviderConfig | null>(raw.getProvider() as never),

  // ── 文件摄入（§14.2） ──
  ingestFiles: (
    paths: string[],
    onProgress: (p: IngestProgress) => void,
    conversationId: string | null,
    folderPath: string | null,
  ) => {
    const channel = new Channel<IngestProgress>();
    channel.onmessage = onProgress;
    return unwrap<IngestResultItem[]>(
      raw.ingestFiles(paths, channel, conversationId, folderPath),
    );
  },
  cancelIngest: (jobId: string) => unwrap<boolean>(raw.cancelIngest(jobId)),

  // ── 文档挂载（`@` 持久化）──
  mountDocument: (conversationId: string, path: string) =>
    unwrap<boolean>(raw.mountDocument(conversationId, path)),
  unmountDocument: (conversationId: string, path: string) =>
    unwrap<boolean>(raw.unmountDocument(conversationId, path)),
  listMountedDocuments: (conversationId: string) =>
    unwrap<MountedDocDto[]>(raw.listMountedDocuments(conversationId)),
  listAllDocuments: () => unwrap<DocumentSummaryDto[]>(raw.listAllDocuments()),
  deleteDocument: (path: string) => unwrap<boolean>(raw.deleteDocument(path)),
  readDocument: (id: string, offset?: number, limit?: number) =>
    unwrap<DocumentContentDto | null>(
      raw.readDocument(id, offset ?? null, limit ?? null),
    ),

  // ── 文件夹操作 + 会话激活集（CONVERSATION-SCOPE.md）──
  /** 新建空文件夹（持久化）。path 如 "/曾国藩专题"。已存在则忽略。 */
  createFolder: (path: string) => unwrap<boolean>(raw.createFolder(path)),
  listFolders: () => unwrap<FolderNodeDto[]>(raw.listFolders()),
  listDocumentsByFolder: (folder: string | null) =>
    unwrap<DocumentSummaryDto[]>(raw.listDocumentsByFolder(folder)),
  moveDocument: (path: string, targetFolder: string | null) =>
    unwrap<boolean>(raw.moveDocument(path, targetFolder)),
  renameFolder: (oldPath: string, newPath: string) =>
    unwrap<boolean>(raw.renameFolder(oldPath, newPath)),
  deleteFolder: (folder: string) => unwrap<number>(raw.deleteFolder(folder)),
  getActiveScope: (conversationId: string) =>
    unwrap<ActiveScopeDto>(raw.getActiveScope(conversationId)),
  setActiveFolders: (conversationId: string, folders: string[]) =>
    unwrap<null>(raw.setActiveFolders(conversationId, folders)),
  setActiveSources: (conversationId: string, sources: string[]) =>
    unwrap<null>(raw.setActiveSources(conversationId, sources)),
  setActiveOntologies: (conversationId: string, ontologies: string[]) =>
    unwrap<null>(raw.setActiveOntologies(conversationId, ontologies)),

  // ── MCP 工具系统（二期 A3） ──
  setMcpServers: (servers: McpServerConfig[]) =>
    unwrap<McpServerStatus[]>(raw.setMcpServers(servers as never)),
  getMcpServers: () => unwrap<McpServerConfig[]>(raw.getMcpServers() as never),
  listMcpTools: () => unwrap<McpToolDef[]>(raw.listMcpTools() as never),

  // ── Skill 系统（决策 20） ──
  /** 列出全部已发现的 skill，合并全局/会话级激活状态。conversationId 为 null 时只返回全局状态。 */
  listSkills: (conversationId: string | null) =>
    unwrap<SkillDto[]>(raw.listSkills(conversationId)),
  /** 导入本地 skill 目录（复制到 ~/.onto-studio/skills/<name>/）。返回 skill name。 */
  importSkillFromDir: (srcPath: string) =>
    unwrap<string>(raw.importSkillFromDir(srcPath)),
  /** 导入 zip skill（解压 + 校验 + 复制）。返回 skill name。 */
  importSkillFromZip: (zipPath: string) =>
    unwrap<string>(raw.importSkillFromZip(zipPath)),
  /** 卸载导入的 skill（仅 imported 可卸载）。 */
  uninstallSkill: (skillName: string) =>
    unwrap<null>(raw.uninstallSkill(skillName)),
  /** 设置会话级 skill enabled 状态（层次 3）。 */
  setSkillConversationEnabled: (
    conversationId: string,
    skillName: string,
    source: SkillSource,
    enabled: boolean,
  ) =>
    unwrap<null>(
      raw.setSkillConversationEnabled(
        conversationId,
        skillName,
        source,
        enabled,
      ),
    ),
  /** 设置全局 skill 禁用状态（层次 2）。 */
  setSkillGloballyDisabled: (skillName: string, disabled: boolean) =>
    unwrap<null>(raw.setSkillGloballyDisabled(skillName, disabled)),

  // ── RAG 检索增强已移除（一期收尾改为 agent 文件工具 + FTS5）──

  // ── 联邦查询（三期）──
  /** 注册数据源（落 SQLite + 热注册到 SessionContext + 探测连接）。 */
  registerDataSource: (config: DataSourceConfig) =>
    unwrap<DataSourceSummary>(raw.registerDataSource(config as never)),
  /** 测试连接（临时注册探查后注销，不落库）。返回表结构快照。 */
  testDataSource: (config: DataSourceConfig) =>
    unwrap<SchemaSnapshot>(raw.testDataSource(config as never)),
  /** 注销数据源（删 SQLite 记录；catalog 随进程留存，重启不恢复——DF54 限制）。 */
  deregisterDataSource: (id: string) =>
    unwrap<null>(raw.deregisterDataSource(id)),
  /** 列出所有已注册数据源（含连接状态/表数）。 */
  listDataSources: () =>
    unwrap<DataSourceSummary[]>(raw.listDataSources() as never),
  /** 取单个数据源配置（编辑用）。 */
  getDataSource: (id: string) =>
    unwrap<DataSourceConfig | null>(raw.getDataSource(id) as never),
  /** 浏览 catalog 下所有表结构（不含样本行）。 */
  browseFederationSchema: (catalog: string) =>
    unwrap<SchemaSnapshot>(raw.browseFederationSchema(catalog)),
  /** 描述单表：列/类型/可空 + 前 5 行样本 + 行数估计。 */
  describeFederationTable: (catalog: string, table: string) =>
    unwrap<TableMeta>(raw.describeFederationTable(catalog, table)),
  /** 执行只读 SQL（三段式 catalog.public.table）。自动追加 LIMIT，30s 超时。 */
  executeFederationQuery: (sql: string, limit?: number) =>
    unwrap<QueryResult>(raw.executeFederationQuery(sql, limit ?? null)),
  /** EXPLAIN：生成执行计划摘要（调试/审计）。 */
  explainFederationQuery: (sql: string) =>
    unwrap<string>(raw.explainFederationQuery(sql)),

  // ── 本体建模（三期：ontology-store）──
  // OntologyPayload 等含 serde_json::Value 字段，specta BigIntForbidden，
  // 故 export/preview/import 用 String（JSON）传输，调用方 JSON.parse。
  /** 列出所有已存储本体（列表页用）。 */
  listOntologies: () => unwrap<OntologySummary[]>(raw.listOntologies()),
  /** 列出指定本体下的全部数据集（按本体隔离）。返回 JSON 字符串。 */
  listOntologyDatasets: (ontologyApiName: string) =>
    unwrap<string>(raw.listOntologyDatasets(ontologyApiName) as never),
  /** 列出指定本体下的全部数据源（按本体隔离）。返回 JSON 字符串。 */
  listOntologyDataSources: (ontologyApiName: string) =>
    unwrap<string>(raw.listOntologyDataSources(ontologyApiName) as never),
  /** 导出本体为 OntologyPayload JSON 字符串。 */
  exportOntology: (apiName: string) =>
    unwrap<string>(raw.exportOntology(apiName)),
  /** 预演导入（dry-run），返回 ImportPreview JSON 字符串。 */
  previewOntologyImport: async (
    payload: OntologyPayload,
    overwrite: string[],
    overwriteDataSources: string[],
  ) => {
    const tStr = performance.now();
    const json = JSON.stringify(payload);
    const tInvoke = performance.now();
    const data = await unwrap<string>(
      raw.previewOntologyImport(json, overwrite, overwriteDataSources),
    );
    console.log(
      `[ipc] preview stringify ${(tInvoke - tStr).toFixed(1)}ms | invoke+unwrap ${(performance.now() - tInvoke).toFixed(1)}ms`,
    );
    return data;
  },
  /** 执行导入（DAG 落库），返回 ImportResult JSON 字符串。 */
  importOntology: async (
    payload: OntologyPayload,
    overwrite: string[],
    overwriteDataSources: string[],
  ) => {
    const tStr = performance.now();
    const json = JSON.stringify(payload);
    const tInvoke = performance.now();
    const data = await unwrap<string>(
      raw.importOntology(json, overwrite, overwriteDataSources),
    );
    console.log(
      `[ipc] import stringify ${(tInvoke - tStr).toFixed(1)}ms | invoke+unwrap ${(performance.now() - tInvoke).toFixed(1)}ms`,
    );
    return data;
  },
  /** 删除本体及其全部子表（硬删，级联清子表；dataset/data_source 不删）。 */
  deleteOntology: (apiName: string) =>
    unwrap<boolean>(raw.deleteOntology(apiName)),
  /** 列出本体变更历史（git commit log 式，revision 倒序）。 */
  listOntologyChangelog: (apiName: string) =>
    unwrap<OntologyChangelog[]>(raw.listOntologyChangelog(apiName)),
  /** 读取本体设计宪章（不变点：业务场景 / 本质 / 设计意图 / 补充说明）。 */
  getOntologyCharter: (apiName: string) =>
    unwrap<OntologyCharter>(raw.getOntologyCharter(apiName)),
  /** 写入/更新本体设计宪章（只有用户明确要求调整时才调用）。 */
  setOntologyCharter: (apiName: string, charter: OntologyCharter) =>
    unwrap<null>(raw.setOntologyCharter(apiName, charter)),
};
