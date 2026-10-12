// UI 层：ModelSettingsPanel.tsx（设置重构 ①）
// 模型提供商配置独立面板：从 SettingsView 拆出（原“模型提供商”Section +
// 高级折叠整块原样迁移），数据源/保存逻辑不变。
// 形态：居中弹窗（非全屏覆盖）——聊天中 ⌘M 或 NavRail 左下角菜单打开，
// 改完即关，不打断对话上下文。设置总视图已退役，本面板是模型配置唯一编辑源。

import { useEffect, useState } from "react"
import { useProvider, useSetProvider, useVerifierProvider, useSetVerifierProvider } from "@/hooks/useProvider"
import { PROVIDER_PRESETS, presetIndexForProvider, type ProviderPreset } from "@/lib/domain"
import type { ProviderKind, InputType, ReasoningLevel } from "@/lib/domain"
import { Check, ChevronDown, Eye, EyeOff, Loader2, X } from "lucide-react"

/** 居中弹窗外壳：遮罩点击关闭，内部滚动。 */
export function ModelSettingsModal({ onClose }: { onClose: () => void }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 max-md:p-2">
      <div className="absolute inset-0 bg-black/40" onClick={onClose} />
      <div className="relative flex max-h-[85vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-border bg-bg shadow-2xl max-md:max-h-[92vh]">
        <div className="flex shrink-0 items-center justify-between border-b border-border px-5 py-3">
          <h2 className="text-base font-semibold">模型设置</h2>
          <button
            onClick={onClose}
            className="rounded p-1.5 text-fg-subtle hover:bg-bg-hover hover:text-fg"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>
        <div className="overflow-y-auto px-5 py-4">
          <ModelSettingsPanel />
        </div>
      </div>
    </div>
  )
}

/**
 * 模型设置：建模者 A / 校验者 B 两份完全对等的独立 provider 配置（§4.4）。
 * A（provider 键）= 对话 + 本体建模共用；B（verifier_provider 键）= 校验对抗
 * 评审/复核专用通道，建议配置不同厂商——独立性档位自动升「完整」。
 */
export function ModelSettingsPanel() {
  return (
    <div>
      <ProviderForm role="modeler" />
      <ProviderForm role="verifier" />
    </div>
  )
}

/** 单角色 provider 表单体（role: modeler=建模者 / verifier=校验者）。 */
function ProviderForm({ role }: { role: "modeler" | "verifier" }) {
  const isVerifier = role === "verifier"
  const aQ = useProvider()
  const bQ = useVerifierProvider()
  const provider = isVerifier ? bQ.data : aQ.data
  const isLoading = isVerifier ? bQ.isLoading : aQ.isLoading
  const setProvider = isVerifier ? useSetVerifierProvider() : useSetProvider()
  const title = isVerifier ? "校验者（B）" : "建模者（A）· 对话 + 本体建模"
  const roleHint = isVerifier
    ? "Verifier · 建议配置不同厂商的模型（如 A=火山方舟 / B=DeepSeek），独立性档位升「完整」"
    : "Modeler · 对话与本体建模共用的模型配置"

  const [kind, setKind] = useState<ProviderKind>("openai_compatible")
  const [apiKey, setApiKey] = useState("")
  const [baseUrl, setBaseUrl] = useState("")
  const [model, setModel] = useState("")
  const [contextWindow, setContextWindow] = useState("")
  const [preamble, setPreamble] = useState("")
  const [showKey, setShowKey] = useState(false)
  const [presetIdx, setPresetIdx] = useState(0)
  const [saved, setSaved] = useState(false)
  // 新增采样/兼容性/增强字段
  const [temperature, setTemperature] = useState("")
  const [maxTokens, setMaxTokens] = useState("")
  const [topP, setTopP] = useState("")
  const [supportsDeveloperRole, setSupportsDeveloperRole] = useState<"" | "true" | "false">("")
  const [supportsReasoningEffort, setSupportsReasoningEffort] = useState<"" | "true" | "false">("")
  const [supportsImage, setSupportsImage] = useState(false)
  const [reasoning, setReasoning] = useState<ReasoningLevel>("off")
  const [extraHeaders, setExtraHeaders] = useState("")
  const [showAdvanced, setShowAdvanced] = useState(false)

  // 从已存 provider 回填
  useEffect(() => {
    if (provider) {
      setKind(provider.kind)
      setApiKey(provider.api_key)
      setBaseUrl(provider.base_url ?? "")
      setModel(provider.model)
      // 上下文窗口以 K（千 token）为单位显示：1000000 tok → "1000"
      setContextWindow(
        provider.context_window != null
          ? String(Math.round(provider.context_window / 1000))
          : "",
      )
      setPreamble(provider.preamble ?? "")
      // 新字段回填
      setTemperature(provider.temperature != null ? String(provider.temperature) : "")
      setMaxTokens(provider.max_tokens != null ? String(provider.max_tokens) : "")
      setTopP(provider.top_p != null ? String(provider.top_p) : "")
      setSupportsDeveloperRole(
        provider.supports_developer_role == null ? "" : provider.supports_developer_role ? "true" : "false",
      )
      setSupportsReasoningEffort(
        provider.supports_reasoning_effort == null ? "" : provider.supports_reasoning_effort ? "true" : "false",
      )
      setSupportsImage(provider.input_types?.some((t) => t === "image") ?? false)
      setReasoning(provider.reasoning ?? "off")
      setExtraHeaders(
        provider.extra_headers
          ? Object.entries(provider.extra_headers)
              .map(([k, v]) => `${k}: ${v}`)
              .join("\n")
          : "",
      )
      // 匹配 preset（按 kind；openai_compatible 按 base_url 消歧义，见 presetIndexForProvider）
      setPresetIdx(presetIndexForProvider(provider))
    }
  }, [provider])

  const onPresetChange = (idx: number) => {
    const p: ProviderPreset = PROVIDER_PRESETS[idx]
    setPresetIdx(idx)
    setKind(p.kind)
    // 有默认 base URL 的预设（Qwen/Ollama 等兼容端点）预填，避免请求打到 rig 默认
    //（openai::Client 默认 = api.openai.com，Qwen 会 404）；无默认的留空用 rig 内置
    setBaseUrl(p.defaultBaseUrl || "")
    // 切 provider 清空手动窗口，回到自动探测（避免把上一家的窗口带到新模型）
    setContextWindow("")
    if (p.models.length && !p.models.includes(model)) setModel(p.models[0])
    // 同步多模态能力声明
    setSupportsImage(p.supportsImage ?? false)
  }

  const save = async () => {
    const cw = contextWindow.trim()
    const mt = maxTokens.trim()
    const t = temperature.trim()
    const tp = topP.trim()
    const eh = extraHeaders.trim()
    // 解析 extraHeaders（“key: value” 每行一个）
    let headers: Record<string, string> | null = null
    if (eh) {
      headers = {}
      for (const line of eh.split("\n")) {
        const i = line.indexOf(":")
        if (i > 0) headers[line.slice(0, i).trim()] = line.slice(i + 1).trim()
      }
      if (Object.keys(headers).length === 0) headers = null
    }
    const inputTypes: InputType[] = supportsImage ? ["text", "image"] : ["text"]
    await setProvider.mutateAsync({
      kind,
      api_key: apiKey,
      base_url: baseUrl || null,
      model,
      temperature: t ? Number(t) : null,
      max_tokens: mt ? Number(mt) : undefined,
      top_p: tp ? Number(tp) : null,
      supports_developer_role:
        supportsDeveloperRole === "" ? null : supportsDeveloperRole === "true",
      supports_reasoning_effort:
        supportsReasoningEffort === "" ? null : supportsReasoningEffort === "true",
      input_types: inputTypes,
      // 输入单位是 K（千 token）：1000 = 1000000 tok ≈ 1M
      context_window: cw && Number(cw) > 0 ? Math.round(Number(cw) * 1000) : undefined,
      preamble: preamble.trim() || null,
      extra_headers: headers,
      reasoning,
    })
    setSaved(true)
    setTimeout(() => setSaved(false), 2000)
  }

  if (isLoading) {
    return <div className="flex h-40 items-center justify-center"><Loader2 className="animate-spin text-fg-subtle" /></div>
  }

  return (
    <div className="mb-6 border-b border-border pb-6 last:mb-0 last:border-b-0 last:pb-0">
      <div className="mb-3 flex items-baseline justify-between gap-2">
        <h3 className="text-sm font-semibold">{title}</h3>
        <span className="text-[11px] text-fg-subtle">{roleHint}</span>
      </div>

      <MField label="提供商">
        <select
          value={presetIdx}
          onChange={(e) => onPresetChange(Number(e.target.value))}
          className="m-input"
        >
          {PROVIDER_PRESETS.map((p, i) => (
            <option key={p.label} value={i}>{p.label}</option>
          ))}
        </select>
      </MField>

      <MField label="API Key">
        <div className="relative">
          <input
            type={showKey ? "text" : "password"}
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="sk-…"
            className="m-input pr-9"
          />
          <button
            onClick={() => setShowKey(!showKey)}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-subtle hover:text-fg"
          >
            {showKey ? <EyeOff size={15} /> : <Eye size={15} />}
          </button>
        </div>
      </MField>

      <MField label="Base URL" hint="留空使用 provider 默认">
        <input
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder={PROVIDER_PRESETS[presetIdx].defaultBaseUrl || "https://…"}
          className="m-input"
        />
      </MField>

      <MField label="模型">
        {PROVIDER_PRESETS[presetIdx].models.length > 0 ? (
          <div className="flex flex-wrap gap-1.5">
            {PROVIDER_PRESETS[presetIdx].models.map((m) => (
              <button
                key={m}
                onClick={() => setModel(m)}
                className={`rounded-md border px-2.5 py-1 text-xs ${
                  model === m ? "border-accent bg-accent/10 text-accent" : "border-border hover:bg-bg-hover"
                }`}
              >
                {m}
              </button>
            ))}
            <input
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder="或自定义模型名"
              className="m-input min-w-[160px] flex-1"
            />
          </div>
        ) : (
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="模型名，如 gpt-4o"
            className="m-input"
          />
        )}
      </MField>

      <MField
        label="上下文窗口（K tokens）"
        hint="单位 K（千 token）：1000 = 1M、128 = 128K。留空自动探测（OpenRouter / Anthropic / Gemini 读官方字段；DeepSeek 等内置官方窗口；未知模型保守用 100K）"
      >
        <input
          type="number"
          value={contextWindow}
          onChange={(e) => setContextWindow(e.target.value)}
          placeholder="自动探测（默认 100）"
          min={1}
          className="m-input"
        />
      </MField>

      <MField label="系统人设（preamble）" hint="可选。系统提示词，会拼在 Skill 段之前（保 prefix cache）。留空则仅用 Skill 段。">
        <textarea
          value={preamble}
          onChange={(e) => setPreamble(e.target.value)}
          placeholder="例如：你是一个严谨的知识工作助手，回答前先检索已挂载文档…"
          rows={3}
          className="m-input resize-y font-mono text-xs"
        />
      </MField>

      {/* ── 高级设置（折叠）── */}
      <div className="mt-4">
        <button
          onClick={() => setShowAdvanced(!showAdvanced)}
          className="flex items-center gap-1.5 text-xs font-medium text-fg-muted hover:text-fg"
        >
          <ChevronDown
            size={14}
            className={`transition-transform ${showAdvanced ? "rotate-180" : ""}`}
          />
          高级设置（采样 / 兼容性 / 思考 / Headers）
        </button>
        {showAdvanced && (
          <div className="mt-3 space-y-4 border-l-2 border-border pl-4">
            <MField label="温度（temperature）" hint="控制输出随机性。0 = 确定性，知识工作台常设 0.1-0.3。留空用 provider 默认。">
              <input
                type="number"
                step="0.1"
                min={0}
                max={2}
                value={temperature}
                onChange={(e) => setTemperature(e.target.value)}
                placeholder="留空 = provider 默认"
                className="m-input"
              />
            </MField>
            <MField label="最大输出 tokens" hint="区别于上下文窗口（输入）。Anthropic 默认 4096；留空 = 不设。">
              <input
                type="number"
                value={maxTokens}
                onChange={(e) => setMaxTokens(e.target.value)}
                placeholder="留空 = provider 默认"
                min={1}
                className="m-input"
              />
            </MField>
            <MField label="Top P" hint="nucleus sampling。留空 = 不设。">
              <input
                type="number"
                step="0.1"
                min={0}
                max={1}
                value={topP}
                onChange={(e) => setTopP(e.target.value)}
                placeholder="留空 = 不设"
                className="m-input"
              />
            </MField>
            <MField label="深度思考级别" hint="开启后模型会先推理再回答。Composer 的「深度思考」开关会运行时覆盖为 High。">
              <div className="flex gap-1.5">
                {(["off", "low", "medium", "high"] as ReasoningLevel[]).map((r) => {
                  const labels: Record<ReasoningLevel, string> = { off: "关闭", low: "低", medium: "中", high: "高" }
                  return (
                    <button
                      key={r}
                      onClick={() => setReasoning(r)}
                      className={`rounded-md border px-2.5 py-1 text-xs ${
                        reasoning === r ? "border-accent bg-accent/10 text-accent" : "border-border hover:bg-bg-hover"
                      }`}
                    >
                      {labels[r]}
                    </button>
                  )
                })}
              </div>
            </MField>
            <MField label="支持 developer role" hint="Ollama/vLLM 等不认 developer role，需降级为 system。留空 = 按 provider 默认。">
              <select
                value={supportsDeveloperRole}
                onChange={(e) => setSupportsDeveloperRole(e.target.value as "" | "true" | "false")}
                className="m-input"
              >
                <option value="">默认（按 provider）</option>
                <option value="true">支持</option>
                <option value="false">不支持（降级为 system）</option>
              </select>
            </MField>
            <MField label="支持 reasoning_effort" hint="兼容端点不认 reasoning_effort 参数时需 drop。留空 = 按 provider 默认。">
              <select
                value={supportsReasoningEffort}
                onChange={(e) => setSupportsReasoningEffort(e.target.value as "" | "true" | "false")}
                className="m-input"
              >
                <option value="">默认（按 provider）</option>
                <option value="true">支持</option>
                <option value="false">不支持（drop）</option>
              </select>
            </MField>
            <MField label="图片输入" hint="模型是否支持多模态图片输入。UI 据此显隐图片上传按钮。">
              <label className="flex items-center gap-2 text-xs">
                <input
                  type="checkbox"
                  checked={supportsImage}
                  onChange={(e) => setSupportsImage(e.target.checked)}
                  className="accent-accent"
                />
                支持图片输入（多模态）
              </label>
            </MField>
            <MField label="自定义 HTTP Headers" hint="每行一个，格式 `key: value`。用于代理、OpenRouter X-Title、Anthropic anthropic_betas 等。">
              <textarea
                value={extraHeaders}
                onChange={(e) => setExtraHeaders(e.target.value)}
                placeholder={`X-Title: onto-studio\nHTTP-Referer: https://onto-studio.app`}
                rows={3}
                className="m-input resize-y font-mono text-xs"
              />
            </MField>
          </div>
        )}
      </div>

      <div className="mt-4 flex items-center gap-3">
        <button
          onClick={save}
          disabled={!apiKey || !model || setProvider.isPending}
          className="flex items-center gap-1.5 rounded-md bg-accent px-4 py-2 text-sm font-medium text-accent-fg hover:opacity-90 disabled:opacity-40"
        >
          {setProvider.isPending ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
          保存
        </button>
        {saved && <span className="text-xs text-accent">已保存</span>}
      </div>

      <style>{`
        .m-input {
          width: 100%;
          border-radius: 6px;
          border: 1px solid var(--border);
          background: var(--bg-elevated);
          padding: 0.5rem 0.75rem;
          font-size: 0.875rem;
          outline: none;
        }
        .m-input:focus { border-color: var(--accent); }
      `}</style>
    </div>
  )
}

function MField({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="mb-3">
      <label className="mb-1 flex items-center gap-2 text-xs font-medium text-fg-muted">
        {label}
        {hint && <span className="font-normal text-fg-subtle">· {hint}</span>}
      </label>
      {children}
    </div>
  )
}
