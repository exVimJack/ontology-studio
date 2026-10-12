// 宪章 AI 建议（P0 纯前端）： CharterPanel 编辑模式的「AI 建议」。
//
// 红线（设计宪章用户主权，见 PROGRESS 决策）：
//   1. AI 永不直接写宪章——本模块只返回建议，采纳/保存由用户在面板操作
//   2. AI 不虚构业务事实——缺失信息返回 null + 提问（questions），不允许编造
//   3. 建议必须显式采纳——逐字段对比卡片，用户点「采纳」才进草稿
//
// 调用方式：直接 fetch provider 的 OpenAI 兼容 chat/completions
// （anthropic 走 /v1/messages）。provider 配置经 ipc.getProvider() 读取，
// api_key 前端可见（对齐 useHasProvider 的既有约定）。

import type { ProviderConfig, TtlCharter } from "@/lib/domain";
import { DEFAULT_BASE_URLS } from "@/lib/domain";
import { ipc } from "@/lib/ipc/commands";

/** 建议模式：起草（空字段起稿）/ 补全（只填空缺）/ 完善（润色既有文案）/ 查漏（找矛盾与缺口）。 */
export type CharterSuggestMode = "draft" | "complete" | "refine" | "gap";

export const CHARTER_SUGGEST_MODES: {
  value: CharterSuggestMode;
  label: string;
  hint: string;
}[] = [
  { value: "draft", label: "起草", hint: "四字段为空时基于本体结构起稿" },
  { value: "complete", label: "补全", hint: "只填空缺字段，不动已填内容" },
  { value: "refine", label: "完善", hint: "润色既有文案，不改含义" },
  { value: "gap", label: "查漏", hint: "找矛盾、缺口与未覆盖的约束" },
];

/** AI 建议结果。null = 该字段无建议（不得臆造）；questions = 需要用户回答的业务问题。 */
export interface CharterSuggestion {
  business_scenario: string | null;
  business_essence: string | null;
  questions: string[];
}

type CharterTextField = Exclude<keyof TtlCharter, "updated_by" | "updated_at">;

const FIELD_META: { key: CharterTextField; label: string; semantic: string }[] =
  [
    {
      key: "business_scenario",
      label: "业务场景",
      semantic: "服务于什么业务目标、谁用、解决什么问题",
    },
    {
      key: "business_essence",
      label: "业务本质",
      semantic: "核心业务对象/状态/关系/动态行为的一句话本质概括",
    },
  ];

function systemPrompt(): string {
  const fieldDesc = FIELD_META.map(
    (f) => `- ${f.key}（${f.label}）：${f.semantic}`,
  ).join("\n");
  return [
    "你是本体建模助手，帮助建模者撰写「设计宪章」（ontology design charter）。",
    "设计宪章记录本体的业务本质说明，是不变点，只能由用户确认后写入。",
    "",
    "两个字段语义（设计意图/补充说明偏开发视角，不对业务呈现，无需产出）：",
    fieldDesc,
    "",
    "硬性规则：",
    "1. 你只能基于「用户草稿」和「本体结构上下文」整理与归纳；",
    "2. 严禁虚构业务事实——上下文和草稿不足以支撑的字段，输出 null，并在 questions 里提出具体问题让用户回答；",
    "3. 不要把本体里的类名/属性名罗列当成业务场景——要概括业务目标与链路；",
    "4. 输出严格 JSON，不要 markdown 代码块，格式：",
    '   {"business_scenario": string|null, "business_essence": string|null, "questions": string[]}',
  ].join("\n");
}

function modeInstruction(mode: CharterSuggestMode): string {
  switch (mode) {
    case "draft":
      return "模式：起草。草稿为空，请基于本体结构上下文给出两字段初稿建议；业务目标等本体结构推不出的信息一律 null + 提问。";
    case "complete":
      return "模式：补全。只对空缺字段给建议；已有内容的字段输出 null（不要改写用户已写的文字）。";
    case "refine":
      return "模式：完善。对已有文字做措辞与结构润色，保持原含义不变，不添加新业务事实；没有可完善的字段输出 null。";
    case "gap":
      return "模式：查漏。检查草稿与本体结构之间的矛盾、缺口（如场景与本质不一致、本质未覆盖核心对象），并必须列出发现的问题。";
  }
}

function userPrompt(
  mode: CharterSuggestMode,
  draft: TtlCharter,
  entityContext: string,
): string {
  const draftText = FIELD_META.map(
    (f) => `${f.label}（${f.key}）：${draft[f.key]?.trim() || "（空）"}`,
  ).join("\n");
  return [
    modeInstruction(mode),
    "",
    "── 用户草稿 ──",
    draftText,
    "",
    "── 本体结构上下文（类/属性的中文 label 与注释摘要）──",
    entityContext || "（无）",
    "",
    "请输出严格 JSON。",
  ].join("\n");
}

/** 解析模型返回：剥 markdown 代码块 + 提取首个 JSON 对象。 */
function parseSuggestion(text: string): CharterSuggestion {
  let raw = text.trim();
  const fence = raw.match(/```(?:json)?\s*([\s\S]*?)```/);
  if (fence) raw = fence[1].trim();
  const start = raw.indexOf("{");
  const end = raw.lastIndexOf("}");
  if (start < 0 || end <= start) {
    throw new Error("模型未返回 JSON，无法解析建议");
  }
  const obj = (() => {
    try {
      return JSON.parse(raw.slice(start, end + 1)) as Record<string, unknown>;
    } catch {
      throw new Error(
        `AI 输出 JSON 解析失败（可能被截断，可重试或换非推理模型）：${raw.slice(start, start + 120)}`,
      );
    }
  })();
  const str = (v: unknown): string | null => {
    if (typeof v !== "string") return null;
    const t = v.trim();
    return t.length > 0 ? t : null;
  };
  const questions = Array.isArray(obj.questions)
    ? obj.questions.filter(
        (q): q is string => typeof q === "string" && q.trim().length > 0,
      )
    : [];
  return {
    business_scenario: str(obj.business_scenario),
    business_essence: str(obj.business_essence),
    questions,
  };
}

/** provider 实际请求 base URL（base_url 优先，其次按 kind 的默认值）。 */
function resolveBaseUrl(provider: ProviderConfig): string {
  if (provider.base_url && provider.base_url.trim()) {
    return provider.base_url.replace(/\/+$/, "");
  }
  const fallback = DEFAULT_BASE_URLS[provider.kind] ?? "";
  return fallback.replace(/\/+$/, "");
}

/** POST JSON 并返回响应体。走 Rust http_post_json 代理——
 *  webview fetch 受 CORS 预检限制（部分网关 OPTIONS 404 → "Failed to Fetch"，
 *  实测火山方舟网关），LLM 请求统一绕道 Rust reqwest。非 2xx 抛错。 */
async function postJson(
  url: string,
  headers: Record<string, string>,
  body: unknown,
): Promise<string> {
  const r = await ipc.httpPostJson(url, headers, JSON.stringify(body));
  if (r.status < 200 || r.status >= 300) {
    throw new Error(
      `LLM 请求失败（HTTP ${r.status}）：${r.body.slice(0, 300)}`,
    );
  }
  return r.body;
}

async function callOpenAiCompatible(
  provider: ProviderConfig,
  system: string,
  user: string,
): Promise<string> {
  let base = resolveBaseUrl(provider);
  if (provider.kind === "gemini") {
    // Gemini 的 OpenAI 兼容层
    base = `${base}/v1beta/openai`;
  } else if (provider.kind === "ollama" && !/\/v\d+$/.test(base)) {
    base = `${base}/v1`;
  }
  const raw = await postJson(
    `${base}/chat/completions`,
    { Authorization: `Bearer ${provider.api_key}` },
    {
      model: provider.model,
      temperature: 0.3,
      // 推理模型（如 glm-5.3-flash）reasoning 阶段会消耗大量 token，2048 会截断正文
      max_tokens: 8192,
      messages: [
        { role: "system", content: system },
        { role: "user", content: user },
      ],
    },
  );
  const data = JSON.parse(raw) as {
    choices?: { message?: { content?: string } }[];
  };
  const content = data.choices?.[0]?.message?.content ?? "";
  if (!content.trim()) throw new Error("模型返回空内容");
  return content;
}

async function callAnthropic(
  provider: ProviderConfig,
  system: string,
  user: string,
): Promise<string> {
  const base = resolveBaseUrl(provider);
  const raw = await postJson(
    `${base}/v1/messages`,
    {
      "x-api-key": provider.api_key,
      "anthropic-version": "2023-06-01",
    },
    {
      model: provider.model,
      // 同上：推理模型兼容
      max_tokens: 8192,
      temperature: 0.3,
      system,
      messages: [{ role: "user", content: user }],
    },
  );
  const data = JSON.parse(raw) as { content?: { text?: string }[] };
  const content = (data.content ?? [])
    .map((c) => c.text ?? "")
    .join("")
    .trim();
  if (!content) throw new Error("模型返回空内容");
  return content;
}

/** 请求宪章建议。失败抛 Error（调用方 toast 展示，绝不静默落值）。 */
export async function suggestCharter(
  provider: ProviderConfig,
  input: {
    mode: CharterSuggestMode;
    draft: TtlCharter;
    entityContext: string;
  },
): Promise<CharterSuggestion> {
  const system = systemPrompt();
  const user = userPrompt(input.mode, input.draft, input.entityContext);
  const text =
    provider.kind === "anthropic"
      ? await callAnthropic(provider, system, user)
      : await callOpenAiCompatible(provider, system, user);
  return parseSuggestion(text);
}
