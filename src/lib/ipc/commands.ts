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
  TtlOntologySummary,
  TtlValidation,
  TtlImportResult,
  TtlCharter,
  TtlChangelog,
  TtlPropertyEdit,
  ValidationIssue,
  ValidationReview,
  AutoDisputeItem,
  AutoDisputeState,
  PostArbitrationState,
  CoverageMatrix,
  BReviewOutput,
  AReviseResult,
  SetProviderInput_Deserialize,
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
  getVerifierProvider: () => unwrap<ProviderConfig | null>(raw.getVerifierProvider() as never),
  setVerifierProvider: (input: SetProviderInput) =>
    unwrap<ProviderConfig>(
      raw.setVerifierProvider(input as unknown as SetProviderInput_Deserialize) as never,
    ),

  // ── HTTP 代理（宪章 AI 建议 LLM 直连；绕 webview CORS 预检限制）──
  httpPostJson: (url: string, headers: Record<string, string>, body: string) =>
    unwrap<{ status: number; body: string }>(
      raw.httpPostJson(url, headers, body) as never,
    ),
  /** 当前上下文窗口（token），供会话用量百分比展示。null = 未配置 provider。 */
  getContextWindow: () =>
    unwrap<number | null>(raw.getContextWindow() as never),

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
  listElementEvidence: (standard: string, ontologyKey: string) =>
    unwrap<string>(raw.listElementEvidence(standard, ontologyKey) as never),
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
  // 宪章先行（决策 24）：空骨架本体 + 宪章原子创建，仅宪章向导可达
  createOntologyWithCharter: (
    apiName: string,
    displayName: string,
    description: string,
    charter: OntologyCharter,
  ) =>
    unwrap<null>(
      raw.createOntologyWithCharter(apiName, displayName, description, charter),
    ),

  // ── W3C Turtle 本体（对齐 skill ontology-modeling-w3c）──
  /** 列出所有已存 W3C Turtle 本体。 */
  listOntologyTtl: () => unwrap<TtlOntologySummary[]>(raw.listOntologyTtl()),
  /** 导出指定本体 IRI 的 Turtle 文本。 */
  exportOntologyTtl: (iri: string) =>
    unwrap<string>(raw.exportOntologyTtl(iri)),
  /** 校验 Turtle（dry-run）。 */
  validateOntologyTtl: (ttl: string) =>
    unwrap<TtlValidation>(raw.validateOntologyTtl(ttl)),
  /** 导入 Turtle（best-effort 落库）。 */
  importOntologyTtl: (ttl: string, overwrite: boolean) =>
    unwrap<TtlImportResult>(raw.importOntologyTtl(ttl, overwrite)),
  /** 删除指定本体 IRI（幂等，级联清 charter/changelog）。 */
  deleteOntologyTtl: (iri: string) =>
    unwrap<boolean>(raw.deleteOntologyTtl(iri)),
  /** 执行 SPARQL 查询，返回 SPARQL Results JSON 字符串。 */
  queryOntologySparql: (iri: string, sparql: string) =>
    unwrap<string>(raw.queryOntologySparql(iri, sparql)),
  /** 读取 W3C 本体设计宪章（不变点）。 */
  getOntologyTtlCharter: (iri: string) =>
    unwrap<TtlCharter>(raw.getOntologyTtlCharter(iri)),
  /** 写入/更新 W3C 本体设计宪章（只有用户明确要求调整时才调用）。 */
  setOntologyTtlCharter: (iri: string, charter: TtlCharter) =>
    unwrap<null>(raw.setOntologyTtlCharter(iri, charter)),
  /** 宪章先行（决策 24）：空骨架 W3C 本体 + 宪章原子创建，仅宪章向导可达。 */
  createOntologyTtlWithCharter: (
    iri: string,
    displayName: string,
    charter: TtlCharter,
  ) =>
    unwrap<null>(raw.createOntologyTtlWithCharter(iri, displayName, charter)),
  /** 列出 W3C 本体变更历史（revision 倒序）。 */
  listOntologyTtlChangelog: (iri: string) =>
    unwrap<TtlChangelog[]>(raw.listOntologyTtlChangelog(iri)),
  /** 提交一条 W3C 本体变更日志。返回 revision 序号。 */
  commitOntologyTtlChange: (
    iri: string,
    title: string,
    body: string,
    changeSummary: string,
    conversationId: string | null,
  ) =>
    unwrap<number>(
      raw.commitOntologyTtlChange(
        iri,
        title,
        body,
        changeSummary,
        conversationId,
      ),
    ),

  // ── W3C Turtle 属性结构化编辑（手动编辑 P0：属性/关系，不动类）──
  /** 列出本体全部属性（散块已聚合），属性编辑 Tab 数据源。 */
  listOntologyTtlProperties: (iri: string) =>
    unwrap<TtlPropertyEdit[]>(raw.listOntologyTtlProperties(iri)),
  /** 列出本体全部类的 IRI（domain/range 下拉用）。 */
  listOntologyTtlClassIris: (iri: string) =>
    unwrap<string[]>(raw.listOntologyTtlClassIris(iri)),
  /** 新增属性表单的默认 IRI 前缀（与既有属性 namespace 一致）。 */
  getOntologyTtlDefaultPropertyNs: (iri: string) =>
    unwrap<string>(raw.getOntologyTtlDefaultPropertyNs(iri)),
  /** 编辑既有属性（iri 为身份不可改），返回手术后的完整 TTL。 */
  editOntologyTtlProperty: (iri: string, edit: TtlPropertyEdit) =>
    unwrap<string>(raw.editOntologyTtlProperty(iri, edit)),
  /** 新增属性，返回手术后的完整 TTL。 */
  addOntologyTtlProperty: (iri: string, edit: TtlPropertyEdit) =>
    unwrap<string>(raw.addOntologyTtlProperty(iri, edit)),
  /** 删除属性（被引用时后端拒绝），返回手术后的完整 TTL。 */
  deleteOntologyTtlProperty: (iri: string, propertyIri: string) =>
    unwrap<string>(raw.deleteOntologyTtlProperty(iri, propertyIri)),

  // ── 校验对抗（VALIDATION-DESIGN.md v0.3.1）──
  /** A 对 warning 级 issue 发起答辩（L3 第二层，counterEvidence 机械匹配）。 */
  disputeIssue: (
    issueId: string,
    argument: string,
    counterSourceId: string,
    counterQuote: string,
  ) => unwrap<string>(raw.disputeIssue(issueId, argument, counterSourceId, counterQuote)),
  /** 裁决答辩（B 复核或人工调用；不作为 A 工具暴露）。 */
  resolveDispute: (issueId: string, verdict: string, resolvedBy: string) =>
    unwrap<string>(raw.resolveDispute(issueId, verdict, resolvedBy)),
  /** 自动答辩状态缓存（running + items，切窗回来恢复"进行中"展示）。 */
  getAutoDisputeResult: (ontologyIri: string) =>
    unwrap<AutoDisputeState>(raw.getAutoDisputeResult(ontologyIri)),
  getPostArbitrationState: (ontologyIri: string) =>
    unwrap<PostArbitrationState>(raw.getPostArbitrationState(ontologyIri)),
  runPostArbitrationRevise: (ontologyIri: string) =>
    unwrap<null>(raw.runPostArbitrationRevise(ontologyIri)),
  resolveDisputeLlm: (issueId: string, modelB: string) =>
    unwrap<string>(raw.resolveDisputeLlm(issueId, modelB)),
  /** 答辩提交 + B 自动机器裁决（一步到位，Rust 侧落库，切窗安全）。 */
  disputeAndJudge: (issueId: string, argument: string, counterSourceId: string, counterQuote: string, modelB: string) =>
    unwrap<string>(raw.disputeAndJudge(issueId, argument, counterSourceId, counterQuote, modelB)),
  /** A 自动答辩（对全部 rules 来源 open warning 后端跑 A 决策→提交→B 裁决）。 */
  runAutoDispute: (ontologyIri: string) =>
    unwrap<AutoDisputeItem[]>(raw.runAutoDispute(ontologyIri)),
  /** 查询本体的全部 issue（对抗状态展示 + 报告）。 */
  listValidationIssues: (ontologyIri: string) =>
    unwrap<ValidationIssue[]>(raw.listValidationIssues(ontologyIri)),
  /** 持久化一次 validate 的 issue 快照。 */
  persistValidationIssues: (
    ontologyIri: string,
    validationId: string,
    issues: ValidationIssue[],
  ) => unwrap<number>(raw.persistValidationIssues(ontologyIri, validationId, issues)),
  /** 启动 L4.3 语义评审。 */
  reviewOntology: (ontologyIri: string) =>
    unwrap<ValidationReview>(raw.reviewOntology(ontologyIri)),
  /** A 提交修订（a_reasoning 对 B 复核不可见）。 */
  aRevise: (reviewId: string, aRevision: string, aReasoning: string) =>
    unwrap<ValidationReview>(raw.aRevise(reviewId, aRevision, aReasoning)),
  /** B 复核：confirm=true → resolved；false → escalated。 */
  bRecheck: (reviewId: string, confirm: boolean) =>
    unwrap<ValidationReview>(raw.bRecheck(reviewId, confirm)),
  /** 转人工仲裁。 */
  escalateReview: (reviewId: string) =>
    unwrap<ValidationReview>(raw.escalateReview(reviewId)),
  /** 查询本体的最新 review。 */
  latestValidationReview: (ontologyIri: string) =>
    unwrap<ValidationReview | null>(raw.latestValidationReview(ontologyIri)),
  /** 持久化 B 评审输出（L4.3 第 1 步产物）。 */
  saveBReport: (reviewId: string, bReport: string) =>
    unwrap<ValidationReview>(raw.saveBReport(reviewId, bReport)),
  /** 29 句话覆盖矩阵（全量或增量 diff）。 */
  coverageMatrix: (ontologyIri: string, diffEntities: string[]) =>
    unwrap<CoverageMatrix>(raw.coverageMatrix(ontologyIri, diffEntities)),
  /** 判定 A/B 模型独立性档位。 */
  independenceTier: (
    providerA: string,
    modelA: string,
    providerB: string,
    modelB: string,
  ) => unwrap<string>(raw.independenceTier(providerA, modelA, providerB, modelB)),
  /** 运行 B 语义评审（LLM 编排）。 */
  runBReviewLlm: (modelB: string, inputs: string) =>
    unwrap<BReviewOutput>(raw.runBReviewLlm(modelB, inputs)),
  /** 运行 A 修订（LLM 编排）。 */
  runAReviseLlm: (modelA: string, ttlV1: string, bSuggestions: string) =>
    unwrap<AReviseResult>(raw.runAReviseLlm(modelA, ttlV1, bSuggestions)),
  /** 运行 B 复核（LLM 编排）。 */
  runBRecheckLlm: (modelB: string, aRevision: string) =>
    unwrap<string>(raw.runBRecheckLlm(modelB, aRevision)),
  /** 一次性触发后端完整语义评审循环（后端驱动：B评审→A修订→B复核→裁决）。
   *  返回 review id；进度轮询 latestValidationReview。 */
  runReviewLoop: (ontologyIri: string, ttlContent: string, charterSummary: string, issuesJson: string) =>
    unwrap<string>(raw.runReviewLoop(ontologyIri, ttlContent, charterSummary, issuesJson)),
};
