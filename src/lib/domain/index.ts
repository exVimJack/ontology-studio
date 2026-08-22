// Domain 层：纯 TS 类型与工具函数（§12.2）
// 与 Rust 侧 specta 生成的类型一一对应；不依赖 IPC 实现，只做 re-export + 业务建模。

import type { SkillDto, SkillSource } from "@/lib/ipc/bindings";
import type { ProviderKind } from "@/lib/ipc/bindings";

// FolderNodeDto / ActiveScopeDto：Rust 侧 `ingest.rs` 定义且 `derive(Type)`，
// 但 `list_folders`/`get_active_scope` 命令未出现在 tauri-specta 生成的 bindings 中
//（预存问题，待排查）。此处手写等价 interface 保证前端编译，字段对齐 Rust 定义。
export interface FolderNodeDto {
  name: string;
  path: string;
  children: FolderNodeDto[];
}
export interface ActiveScopeDto {
  folders: string[];
  documents: string[];
  sources: string[];
  /** 激活的本体 api_name（@OntologyName 引用）。 */
  ontologies: string[];
}

export type {
  AppError,
  ChatStreamChunk,
  ConnectionConfig,
  ConversationRow,
  ConversationSummary,
  CreateConversationInput,
  DataSourceConfig,
  DataSourceKind,
  DataSourceSummary,
  DeleteMessageInput,
  IngestProgress,
  IngestResultItem,
  IngestStage,
  MessageRole,
  MessageRow,
  MessageStatus,
  McpServerConfig,
  McpServerStatus,
  McpToolDef,
  DocumentSummaryDto,
  DocumentContentDto,
  MountedDocDto,
  ProviderConfig,
  ProviderKind,
  InputType,
  ReasoningLevel,
  QueryResult,
  SchemaSnapshot,
  SendMessageInput,
  SetMessageStatusInput,
  SetPinnedInput,
  SetProviderInput,
  SkillDto,
  SkillSource,
  StreamKind,
  TableMeta,
  ToolCallInfo,
  OntologySummary,
  OntologyChangelog,
  OntologyCharter,
} from "@/lib/ipc/bindings";

/**
 * 默认 base URL（仅作 placeholder 提示，实际 None 时用 rig 内置各 provider BASE_URL）。
 * 对齐 Rust 侧 provider.rs / rig 0.41 providers 源码常量。
 */
export const DEFAULT_BASE_URLS: Record<string, string> = {
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com",
  gemini: "https://generativelanguage.googleapis.com",
  deepseek: "https://api.deepseek.com",
  xai: "https://api.x.ai",
  groq: "https://api.groq.com/openai/v1",
  openrouter: "https://openrouter.ai/api/v1",
  ollama: "http://localhost:11434",
  moonshot: "https://api.moonshot.cn/v1",
  qwen: "https://dashscope.aliyuncs.com/compatible-mode/v1",
  zhipu: "https://open.bigmodel.cn/api/paas/v4",
  mistral: "https://api.mistral.ai/v1",
  cohere: "https://api.cohere.ai",
  perplexity: "https://api.perplexity.ai",
};

/** provider 显示名 + 常见模型预设（设置页下拉用）。kind 对齐 bindings ProviderKind。 */
export interface ProviderPreset {
  label: string;
  kind: ProviderKind;
  /** placeholder 提示的默认 base URL；空字符串表示用 rig 默认（用户留空）。 */
  defaultBaseUrl: string;
  models: string[];
  /** 该 provider 默认是否支持图片输入（多模态）。 */
  supportsImage?: boolean;
}

export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    label: "OpenAI",
    kind: "openai",
    defaultBaseUrl: "",
    models: ["gpt-5.5", "gpt-5.5-pro", "gpt-5.6-luna", "gpt-4o"],
    supportsImage: true,
  },
  {
    label: "Anthropic (Claude)",
    kind: "anthropic",
    defaultBaseUrl: "",
    models: ["claude-opus-5", "claude-sonnet-5"],
    supportsImage: true,
  },
  {
    label: "Google Gemini",
    kind: "gemini",
    defaultBaseUrl: "",
    models: ["gemini-3.1-pro", "gemini-3-pro"],
    supportsImage: true,
  },
  {
    label: "DeepSeek",
    kind: "deepseek",
    defaultBaseUrl: "",
    models: ["deepseek-v4-pro", "deepseek-v4-flash"],
  },
  {
    label: "智谱 GLM (Zhipu)",
    kind: "zhipu",
    defaultBaseUrl: "",
    models: ["glm-5.2", "glm-4.6"],
    supportsImage: true,
  },
  {
    label: "通义千问 (Qwen)",
    kind: "openai_compatible",
    defaultBaseUrl: DEFAULT_BASE_URLS.qwen,
    models: ["qwen3.7-max", "qwen3.7-plus", "qwen3.7-flash"],
    supportsImage: true,
  },
  {
    label: "Kimi (月之暗面)",
    kind: "moonshot",
    defaultBaseUrl: "",
    models: ["kimi-k2.5"],
    supportsImage: true,
  },
  {
    label: "OpenRouter",
    kind: "openrouter",
    defaultBaseUrl: "",
    models: [
      "openai/gpt-5.5",
      "anthropic/claude-opus-5",
      "google/gemini-3.1-pro",
    ],
    supportsImage: true,
  },
  {
    label: "Ollama (本地)",
    kind: "ollama",
    defaultBaseUrl: DEFAULT_BASE_URLS.ollama,
    models: ["qwen3:7b", "deepseek-v3:7b", "qwen2.5-vl:7b"],
    supportsImage: true,
  },
  {
    label: "自定义 (OpenAI 兼容)",
    kind: "openai_compatible",
    defaultBaseUrl: "",
    models: [],
  },
];

/** 按 kind 查找预设。 */
export function presetForKind(kind: ProviderKind): ProviderPreset | undefined {
  return PROVIDER_PRESETS.find((p) => p.kind === kind);
}

/** 时间戳（i64 毫秒）→ 相对时间描述。 */
export function relativeTime(ms: number): string {
  const now = Date.now();
  const diff = now - ms;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day} 天前`;
  return new Date(ms).toLocaleDateString("zh-CN");
}

/** 按日分组（侧栏列表用，§20.5）。 */
export type DateGroup = "今天" | "昨天" | "本周" | "本月" | "更早";

export function dateGroup(ms: number): DateGroup {
  const now = new Date();
  const startOfToday = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
  ).getTime();
  const dayMs = 86400000;
  if (ms >= startOfToday) return "今天";
  if (ms >= startOfToday - dayMs) return "昨天";
  if (ms >= startOfToday - 7 * dayMs) return "本周";
  if (ms >= startOfToday - 30 * dayMs) return "本月";
  return "更早";
}

export const DATE_GROUP_ORDER: DateGroup[] = [
  "今天",
  "昨天",
  "本周",
  "本月",
  "更早",
];

// ── Skill 系统（决策 20）──

/** Skill 来源的展示元数据（标签/颜色/图标提示）。 */
export interface SkillSourceMeta {
  label: string;
  /** Tailwind 色系类名前缀，用于 badge 背景/文字。 */
  badgeCls: string;
  /** 是否可卸载（仅 imported）。 */
  removable: boolean;
  /** 是否只读（builtin/external 不可改文件）。 */
  readonly: boolean;
}

export const SKILL_SOURCE_META: Record<SkillSource, SkillSourceMeta> = {
  builtin: {
    label: "内置",
    badgeCls: "bg-accent/10 text-accent border-accent/30",
    removable: false,
    readonly: true,
  },
  imported: {
    label: "已导入",
    badgeCls:
      "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/30",
    removable: true,
    readonly: false,
  },
  "external-read-only": {
    label: "外部",
    badgeCls:
      "bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/30",
    removable: false,
    readonly: true,
  },
  project: {
    label: "项目",
    badgeCls:
      "bg-violet-500/10 text-violet-600 dark:text-violet-400 border-violet-500/30",
    removable: false,
    readonly: true,
  },
};

/** 判断 skill 当前是否在 preamble 中激活（用于展示状态）。 */
export function isSkillActive(s: SkillDto): boolean {
  if (s.globally_disabled) return false;
  if (s.disable_model_invocation) {
    // dmi skill 仅在会话级显式 enabled 时激活
    return s.conversation_enabled === true;
  }
  // 非 dmi：会话级 None=按默认（builtin/external 默认进 preamble），enabled 显式控制
  if (s.conversation_enabled === null) {
    return (
      s.source === "builtin" ||
      s.source === "external-read-only" ||
      s.source === "project"
    );
  }
  return s.conversation_enabled;
}

/** 判断 skill 是否可在会话内切换（dmi/全局禁用仍有条件）。 */
export function canToggleInConversation(s: SkillDto): boolean {
  // 全局禁用时不允许会话内开启（需先在设置页解除全局禁用）
  if (s.globally_disabled) return false;
  return true;
}

// ── 本体建模（三期：ontology-store）───────────────────────────────────
// OntologyPayload / ImportPreview / ImportResult 等含 serde_json::Value 字段，
// specta 2.0-rc 的 serde_json::Number 硬编码 i64/u64 触发 BigIntForbidden，
// 故 command 用 String（JSON）传输，前端 JSON.parse 后用这些手写 interface。
// 字段对齐 Rust `crates/ontology-store/src/payload.rs`。

export interface Capabilities {
  create?: boolean;
  update?: boolean;
  delete?: boolean;
  search?: boolean;
  link?: boolean;
  action?: boolean;
}

export interface BackingMapping {
  dataset_api_name?: string;
  property_mapping?: Record<string, string>;
}

export interface PropertyDef {
  api_name: string;
  display_name: string;
  description?: string;
  data_type: string;
  searchable?: boolean;
  is_primary_key?: boolean;
  is_title_property?: boolean;
  backing_mapping?: BackingMapping;
  vector_config?: unknown;
  confidence?: string;
}

export interface LinkDef {
  api_name: string;
  display_name: string;
  description?: string;
  target_object_type_api_name: string;
  foreign_key_property_api_name?: string;
  cardinality: string;
  weight_property?: string;
  temporal?: boolean;
  confidence?: string;
}

export interface ObjectTypeDef {
  api_name: string;
  display_name: string;
  description?: string;
  primary_key?: string;
  title_property?: string;
  storage_type: string;
  visibility?: string;
  capabilities?: Capabilities;
  properties: PropertyDef[];
  links: LinkDef[];
  confidence?: string;
}

export interface ActionTypeDef {
  api_name: string;
  display_name: string;
  description?: string;
  affected_object_type_api_name: string;
  parameters?: unknown[];
  rules?: unknown[];
  submission_criteria?: unknown[];
  effects?: unknown[];
  ontology_rules?: unknown[];
  risk_level?: string;
  operation_kind?: string;
  batch_enabled?: boolean;
  confidence?: string;
}

export interface DatasetDef {
  api_name: string;
  display_name?: string;
  storage_location?: string;
  partition_config?: unknown;
  source_dataset_api_name?: string;
  data_source_api_name?: string;
  kind?: string;
  is_view?: boolean;
  confidence?: string;
}

export interface DataSourceDef {
  api_name: string;
  display_name: string;
  description?: string;
  connector_type: string;
  connector_config?: unknown;
  credential_id?: string;
  confidence?: string;
}

export interface ObjectTypeGroupDef {
  api_name: string;
  display_name: string;
  description?: string;
  object_type_api_names?: string[];
  confidence?: string;
}

export interface OntologyPayload {
  api_name: string;
  display_name: string;
  description?: string;
  object_types: ObjectTypeDef[];
  action_types?: ActionTypeDef[];
  datasets?: DatasetDef[];
  data_sources?: DataSourceDef[];
  object_type_groups?: ObjectTypeGroupDef[];
}

export interface ImportItemResult {
  api_name: string;
  status: "created" | "skipped" | "overwritten" | "failed";
  error?: string;
}

export interface ImportResult {
  ontology_api_name: string;
  ontology_status: "created" | "existed";
  object_types: ImportItemResult[];
  links_created: number;
  links_skipped: number;
  action_types: ImportItemResult[];
  datasets: ImportItemResult[];
  data_sources: ImportItemResult[];
  object_type_groups: ImportItemResult[];
  errors: string[];
}

export interface ImportPreviewItem {
  api_name: string;
  status: "create" | "skip" | "overwrite" | "fail";
  reason: string;
}

export interface ImportPreview {
  ontology_api_name: string;
  ontology_status: "create" | "skip";
  object_types: ImportPreviewItem[];
  links: ImportPreviewItem[];
  actions: ImportPreviewItem[];
  datasets: ImportPreviewItem[];
  data_sources: ImportPreviewItem[];
  object_type_groups: ImportPreviewItem[];
  warnings: string[];
  errors: string[];
}
