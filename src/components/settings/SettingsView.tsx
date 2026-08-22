// UI 层：SettingsView.tsx（§20.9 一期：单 provider）
// 模型提供商配置 + 外观主题。一期单 provider。

import { useEffect, useState } from "react"
import { useProvider, useSetProvider } from "@/hooks/useProvider"
import { useMcpServers, useSetMcpServers, useMcpTools } from "@/hooks/useMcp"
import { PROVIDER_PRESETS, type ProviderPreset } from "@/lib/domain"
import type { ProviderKind, InputType, ReasoningLevel, McpServerConfig } from "@/lib/domain"
import { useUiStore, applyTheme } from "@/stores/ui-store"
import { Check, ChevronDown, Eye, EyeOff, Loader2, Plus, Trash2, Server, Wrench } from "lucide-react"

export function SettingsView({ onClose }: { onClose: () => void }) {
  const { data: provider, isLoading } = useProvider()
  const setProvider = useSetProvider()
  const theme = useUiStore((s) => s.theme)
  const setTheme = useUiStore((s) => s.setTheme)

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
      setContextWindow(provider.context_window != null ? String(provider.context_window) : "")
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
      // 匹配 preset（按 kind）
      const idx = PROVIDER_PRESETS.findIndex((p) => p.kind === provider.kind)
      setPresetIdx(idx >= 0 ? idx : PROVIDER_PRESETS.length - 1)
    }
  }, [provider])

  const onPresetChange = (idx: number) => {
    const p: ProviderPreset = PROVIDER_PRESETS[idx]
    setPresetIdx(idx)
    setKind(p.kind)
    // base_url 留空用 rig 默认（preset.defaultBaseUrl 仅作 placeholder）
    setBaseUrl("")
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
      context_window: cw && Number(cw) > 0 ? Number(cw) : undefined,
      preamble: preamble.trim() || null,
      extra_headers: headers,
      reasoning,
    })
    setSaved(true)
    setTimeout(() => setSaved(false), 2000)
  }

  if (isLoading) {
    return <div className="flex h-full items-center justify-center"><Loader2 className="animate-spin text-fg-subtle" /></div>
  }

  return (
    <div className="fixed inset-0 z-40 flex flex-col bg-bg max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)]">
      <div className="h-full overflow-y-auto">
        <div className="mx-auto max-w-2xl px-6 py-8 max-md:px-4 max-md:py-4">
          <div className="mb-6 flex items-center justify-between">
            <h1 className="text-xl font-semibold">设置</h1>
            <button onClick={onClose} className="rounded-md border border-border px-3 py-1 text-sm hover:bg-bg-hover">
              关闭
            </button>
          </div>

        {/* ── 模型提供商 ── */}
        <Section title="模型提供商" desc="一期支持单一 provider。API Key 明文存储于本地 SQLite，二期加密。">
          <Field label="提供商">
            <select
              value={presetIdx}
              onChange={(e) => onPresetChange(Number(e.target.value))}
              className="input"
            >
              {PROVIDER_PRESETS.map((p, i) => (
                <option key={p.label} value={i}>{p.label}</option>
              ))}
            </select>
          </Field>

          <Field label="API Key">
            <div className="relative">
              <input
                type={showKey ? "text" : "password"}
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="sk-…"
                className="input pr-9"
              />
              <button
                onClick={() => setShowKey(!showKey)}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-subtle hover:text-fg"
              >
                {showKey ? <EyeOff size={15} /> : <Eye size={15} />}
              </button>
            </div>
          </Field>

          <Field label="Base URL" hint="留空使用 provider 默认">
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder={PROVIDER_PRESETS[presetIdx].defaultBaseUrl || "https://…"}
              className="input"
            />
          </Field>

          <Field label="模型">
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
                  className="input min-w-[160px] flex-1"
                />
              </div>
            ) : (
              <input
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder="模型名，如 gpt-4o"
                className="input"
              />
            )}
          </Field>

          <Field label="上下文窗口（tokens）" hint="留空自动探测（OpenRouter / Anthropic / Gemini 读官方字段；DeepSeek 等内置官方窗口；未知模型保守用 100K）">
            <input
              type="number"
              value={contextWindow}
              onChange={(e) => setContextWindow(e.target.value)}
              placeholder="自动探测（默认 100000）"
              min={1024}
              className="input"
            />
          </Field>

          <Field label="系统人设（preamble）" hint="可选。系统提示词，会拼在 Skill 段之前（保 prefix cache）。留空则仅用 Skill 段。">
            <textarea
              value={preamble}
              onChange={(e) => setPreamble(e.target.value)}
              placeholder="例如：你是一个严谨的知识工作助手，回答前先检索已挂载文档…"
              rows={3}
              className="input resize-y font-mono text-xs"
            />
          </Field>

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
                <Field label="温度（temperature）" hint="控制输出随机性。0 = 确定性，知识工作台常设 0.1-0.3。留空用 provider 默认。">
                  <input
                    type="number"
                    step="0.1"
                    min={0}
                    max={2}
                    value={temperature}
                    onChange={(e) => setTemperature(e.target.value)}
                    placeholder="留空 = provider 默认"
                    className="input"
                  />
                </Field>
                <Field label="最大输出 tokens" hint="区别于上下文窗口（输入）。Anthropic 默认 4096；留空 = 不设。">
                  <input
                    type="number"
                    value={maxTokens}
                    onChange={(e) => setMaxTokens(e.target.value)}
                    placeholder="留空 = provider 默认"
                    min={1}
                    className="input"
                  />
                </Field>
                <Field label="Top P" hint="nucleus sampling。留空 = 不设。">
                  <input
                    type="number"
                    step="0.1"
                    min={0}
                    max={1}
                    value={topP}
                    onChange={(e) => setTopP(e.target.value)}
                    placeholder="留空 = 不设"
                    className="input"
                  />
                </Field>
                <Field label="深度思考级别" hint="开启后模型会先推理再回答。Composer 的「深度思考」开关会运行时覆盖为 High。">
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
                </Field>
                <Field label="支持 developer role" hint="Ollama/vLLM 等不认 developer role，需降级为 system。留空 = 按 provider 默认。">
                  <select
                    value={supportsDeveloperRole}
                    onChange={(e) => setSupportsDeveloperRole(e.target.value as "" | "true" | "false")}
                    className="input"
                  >
                    <option value="">默认（按 provider）</option>
                    <option value="true">支持</option>
                    <option value="false">不支持（降级为 system）</option>
                  </select>
                </Field>
                <Field label="支持 reasoning_effort" hint="兼容端点不认 reasoning_effort 参数时需 drop。留空 = 按 provider 默认。">
                  <select
                    value={supportsReasoningEffort}
                    onChange={(e) => setSupportsReasoningEffort(e.target.value as "" | "true" | "false")}
                    className="input"
                  >
                    <option value="">默认（按 provider）</option>
                    <option value="true">支持</option>
                    <option value="false">不支持（drop）</option>
                  </select>
                </Field>
                <Field label="图片输入" hint="模型是否支持多模态图片输入。UI 据此显隐图片上传按钮。">
                  <label className="flex items-center gap-2 text-xs">
                    <input
                      type="checkbox"
                      checked={supportsImage}
                      onChange={(e) => setSupportsImage(e.target.checked)}
                      className="accent-accent"
                    />
                    支持图片输入（多模态）
                  </label>
                </Field>
                <Field label="自定义 HTTP Headers" hint="每行一个，格式 `key: value`。用于代理、OpenRouter X-Title、Anthropic anthropic_betas 等。">
                  <textarea
                    value={extraHeaders}
                    onChange={(e) => setExtraHeaders(e.target.value)}
                    placeholder={`X-Title: onto-studio\nHTTP-Referer: https://onto-studio.app`}
                    rows={3}
                    className="input resize-y font-mono text-xs"
                  />
                </Field>
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
        </Section>

        {/* ── MCP 工具服务器 ── */}
        <McpSection />

        {/* ── 外观 ── */}
        <Section title="外观" desc="主题与显示偏好">
          <Field label="主题">
            <div className="flex gap-1.5">
              {(["light", "dark", "system"] as const).map((t) => (
                <button
                  key={t}
                  onClick={() => {
                    setTheme(t)
                    applyTheme(t)
                  }}
                  className={`rounded-md border px-3 py-1.5 text-sm ${
                    theme === t ? "border-accent bg-accent/10 text-accent" : "border-border hover:bg-bg-hover"
                  }`}
                >
                  {t === "light" ? "浅色" : t === "dark" ? "深色" : "跟随系统"}
                </button>
              ))}
            </div>
          </Field>
        </Section>

        <div className="mt-8 border-t border-border pt-4 text-xs text-fg-subtle">
          <p>onto-studio · 二期 A3</p>
          <p className="mt-1">知识库 RAG / 快捷键设置将在后续开放。</p>
        </div>
      </div>

      <style>{`
        .input {
          width: 100%;
          border-radius: 6px;
          border: 1px solid var(--border);
          background: var(--bg-elevated);
          padding: 0.5rem 0.75rem;
          font-size: 0.875rem;
          outline: none;
        }
        .input:focus { border-color: var(--accent); }
      `}</style>
      </div>
    </div>
  )
}

function Section({ title, desc, children }: { title: string; desc?: string; children: React.ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="mb-1 text-sm font-semibold">{title}</h2>
      {desc && <p className="mb-3 text-xs text-fg-muted">{desc}</p>}
      <div className="space-y-3">{children}</div>
    </section>
  )
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div>
      <label className="mb-1 flex items-center gap-2 text-xs font-medium text-fg-muted">
        {label}
        {hint && <span className="font-normal text-fg-subtle">· {hint}</span>}
      </label>
      {children}
    </div>
  )
}

/** MCP 工具服务器配置区。 */
function McpSection() {
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
    <Section title="MCP 工具服务器" desc="接入外部 MCP server，工具自动注入对话。stdio 本地进程 / HTTP 远程服务。">
      {servers.length === 0 && (
        <p className="text-xs text-fg-subtle">尚未配置 MCP server。点击下方按钮添加。</p>
      )}

      {servers.map((s) => (
        <div key={s.id} className="rounded-md border border-border p-3 space-y-2">
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
            className="input"
          />
          {s.kind === "stdio" ? (
            <>
              <input
                value={s.command}
                onChange={(e) => update(s.id, (sv) => ({ ...sv, command: e.target.value }))}
                placeholder="命令（如 npx）"
                className="input"
              />
              <input
                value={s.args.join(" ")}
                onChange={(e) =>
                  update(s.id, (sv) => ({ ...sv, args: e.target.value.split(/\s+/).filter(Boolean) }))
                }
                placeholder="参数（空格分隔，如 -y @modelcontextprotocol/server-filesystem /tmp）"
                className="input"
              />
            </>
          ) : (
            <>
              <input
                value={s.url}
                onChange={(e) => update(s.id, (sv) => ({ ...sv, url: e.target.value }))}
                placeholder="URL（如 https://example.com/mcp）"
                className="input"
              />
              <input
                value={s.auth_token ?? ""}
                onChange={(e) =>
                  update(s.id, (sv) => ({ ...sv, auth_token: e.target.value || null }))
                }
                placeholder="Bearer token（可选）"
                className="input"
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
    </Section>
  )
}

/** RAG 知识库配置区。 */
