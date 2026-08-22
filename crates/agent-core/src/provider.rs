//! Provider 配置与构造。
//!
//! 一期 MVP 支持（§九 / 决策 8）：
//!   - OpenAI 官方（走 Responses API）+ 一切 OpenAI 兼容端点（DeepSeek / Ollama /
//!     OpenRouter / Groq / xAI / Mistral / Cohere / Perplexity / Moonshot / 自定义）
//!     —— 共用 rig `openai::Client`，按 kind 切换 `completions_api()` / `responses_api()`
//!     并通过 rig 原生 provider Ext 声明 capability（DeepSeek 的单 chunk tool call 等）
//!   - Anthropic —— 原生多模态强，作为默认 VLM 候选（决策 7）
//!   - Gemini —— Google Generative AI 协议（非 OpenAI 兼容，2026-08 补全）
//!
//! 构造方式统一：`provider::Client::builder().api_key(k).base_url(u).build()`
//! （Rig 0.41 所有 provider 共用 client::Client 泛型壳，见 client/mod.rs）
//!
//! 配置字段对齐 pi-coding-agent 的 models.json schema（api/compat/input/maxTokens/
//! headers/reasoning），降低用户迁移成本。reasoning 支持 4 级（Off/Low/Medium/High）。

use serde::{Deserialize, Serialize};
use specta_typescript::Number;

/// 一期支持的 provider 种类。
///
/// 按 kind 路由到 rig 原生 provider client，而非全部塞进 `openai::Client`——
/// 这样每个 provider 的 capability（`SUPPORTS_TOOLS`/`SUPPORTS_RESPONSE_FORMAT`/
/// `EMITS_COMPLETE_SINGLE_CHUNK_TOOL_CALLS`/`STREAM_INCLUDE_USAGE`）由 rig 正确声明，
/// 避免 DeepSeek 单 chunk tool call 渲染异常、response_format 被错误发送等问题。
///
/// `OpenAiCompatible` 是兜底：未知兼容端点（用户自定义 base_url）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum ProviderKind {
    /// OpenAI 官方端点，走 Responses API（rig `openai::Client` 默认）
    #[serde(rename = "openai")]
    OpenAi,
    /// Anthropic（Claude 系）
    #[serde(rename = "anthropic")]
    Anthropic,
    /// Google Gemini（Generative AI 协议，非 OpenAI 兼容）
    #[serde(rename = "gemini")]
    Gemini,
    /// DeepSeek（OpenAI 兼容，单 chunk tool call + 不支持 response_format）
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// xAI Grok
    #[serde(rename = "xai")]
    Xai,
    /// Groq（超低延迟推理）
    #[serde(rename = "groq")]
    Groq,
    /// OpenRouter（多模型聚合）
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// Ollama（本地）
    #[serde(rename = "ollama")]
    Ollama,
    /// Moonshot / Kimi
    #[serde(rename = "moonshot")]
    Moonshot,
    /// 智谱 GLM（Z.AI，rig `zai` provider，OpenAI 兼容协议 + thinking 参数）
    #[serde(rename = "zhipu")]
    Zhipu,
    /// Mistral
    #[serde(rename = "mistral")]
    Mistral,
    /// Cohere
    #[serde(rename = "cohere")]
    Cohere,
    /// Perplexity（在线搜索，不支持工具）
    #[serde(rename = "perplexity")]
    Perplexity,
    /// 兜底：未知 OpenAI 兼容端点（用户自定义 base_url）
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

impl ProviderKind {
    /// 是否走 OpenAI Completions API（`/chat/completions`）。
    ///
    /// OpenAI 官方走 Responses API；其余 OpenAI 兼容端点只支持 Completions。
    /// Gemini / Anthropic 走各自原生协议，不在此列。
    pub fn uses_completions_api(self) -> bool {
        match self {
            Self::OpenAi => false,
            Self::DeepSeek | Self::Xai | Self::Groq | Self::OpenRouter
            | Self::Ollama | Self::Moonshot | Self::Zhipu | Self::Mistral | Self::Cohere
            | Self::Perplexity | Self::OpenAiCompatible => true,
            Self::Anthropic | Self::Gemini => false,
        }
    }

    /// 是否为 OpenAI 协议族（Completions 或 Responses）。
    /// 用于 reasoning_params 等按协议族分派逻辑。
    pub fn is_openai_family(self) -> bool {
        matches!(
            self,
            Self::OpenAi | Self::DeepSeek | Self::Xai | Self::Groq
                | Self::OpenRouter | Self::Ollama | Self::Moonshot | Self::Zhipu
                | Self::Mistral | Self::Cohere | Self::Perplexity
                | Self::OpenAiCompatible
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::DeepSeek => "deepseek",
            Self::Xai => "xai",
            Self::Groq => "groq",
            Self::OpenRouter => "openrouter",
            Self::Ollama => "ollama",
            Self::Moonshot => "moonshot",
            Self::Zhipu => "zhipu",
            Self::Mistral => "mistral",
            Self::Cohere => "cohere",
            Self::Perplexity => "perplexity",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai" | "openai_official" | "openai-official" => Some(Self::OpenAi),
            // 历史值 "openai_compatible"（一期曾用作统一兼容 kind）兆底
            "openai_compatible" | "openai-compatible" => Some(Self::OpenAiCompatible),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "deepseek" => Some(Self::DeepSeek),
            "xai" => Some(Self::Xai),
            "groq" => Some(Self::Groq),
            "openrouter" => Some(Self::OpenRouter),
            "ollama" => Some(Self::Ollama),
            "moonshot" | "kimi" => Some(Self::Moonshot),
            "zhipu" | "glm" | "zai" => Some(Self::Zhipu),
            "mistral" => Some(Self::Mistral),
            "cohere" => Some(Self::Cohere),
            "perplexity" => Some(Self::Perplexity),
            _ => None,
        }
    }
}

/// 模型输入模态（UI 据此显隐图片上传）。对齐 pi `input` 字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    Text,
    Image,
}

/// 深度思考级别（对齐 pi thinkingLevelMap 的简化版）。
///
/// - Off：不启用（默认）
/// - Low / Medium / High：按 provider 映射为 `reasoning_effort` 或 `thinking.budget_tokens`
///
/// 不引入 pi 的 minimal/xhigh/max 七级——onto-studio 是知识工作台非编码 agent，
/// 三档足够；后续可扩。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReasoningLevel {
    #[default]
    Off,
    Low,
    Medium,
    High,
}


impl ReasoningLevel {
    /// 是否启用（Off 为 false）
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Provider 配置（前端设置页填写，存 SQLite；敏感字段见 §20.9 加密存）。
///
/// 一期：单 provider。二期：多 provider 矩阵。
///
/// 字段分四组：
///   1. **连接**：kind / api_key / base_url / model
///   2. **采样**：temperature / max_tokens / top_p（rig AgentBuilder 原生）
///   3. **兼容性**：supports_developer_role / supports_reasoning_effort / input_types
///      （对齐 pi compat，Ollama/vLLM 等需声明）
///   4. **增强**：context_window / preamble / extra_headers / reasoning
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ProviderConfig {
    // ── 连接 ──
    pub kind: ProviderKind,
    /// API Key（明文，一期；二期落 Rust 侧加密存储）
    pub api_key: String,
    /// 自定义 base URL。None 则用 provider 默认（rig 内置各 provider 的 BASE_URL 常量）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// 模型名，如 "gpt-4o" / "claude-sonnet-4-5" / "deepseek-chat" / "gemini-2.5-pro"
    pub model: String,

    // ── 采样参数（rig AgentBuilder 原生，None 不设用 provider 默认） ──
    /// 采样温度。知识工作台常需低温度（factual 输出）。None 用 provider 默认。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub temperature: Option<f64>,
    /// 最大输出 token 数。区别于 context_window（输入窗口）。
    /// None 时 Anthropic 用 provider 默认（4096）；OpenAI 不设（无上限）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[specta(type = Number)]
    pub max_tokens: Option<u64>,
    /// nucleus sampling。None 不设。走 additional_params 透传。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub top_p: Option<f64>,

    // ── 兼容性声明（对齐 pi compat；仅 OpenAI 兼容族生效） ──
    /// 是否支持 `developer` role。false 时降级为 `system` message（Ollama/vLLM 等）。
    /// None 用 kind 默认（OpenAI 官方 true，兼容端点 false）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_developer_role: Option<bool>,
    /// 是否支持 `reasoning_effort` 参数。false 时 drop（兼容端点不认）。
    /// None 用 kind 默认。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_reasoning_effort: Option<bool>,
    /// 模型支持输入模态。默认 [Text]；多模态模型填 [Text, Image]。
    #[serde(default = "default_input_types")]
    pub input_types: Vec<InputType>,

    // ── 增强 ──
    /// 模型上下文窗口（token 数）。可选，用户显式覆盖。
    ///
    /// 优先级最高（见 context_window::resolve_context_window）：
    ///   用户配置 > 官方元数据探测（按 kind 分派）> 内置已知模型表 > 默认 100K
    /// None 时自动探测（OpenRouter/Anthropic/Gemini 读官方字段；
    /// DeepSeek 等无官方元数据的模型走内置表）。
    /// 探测全程超时+错误保护，不影响对话主流程；真正的安全网是运行时
    /// usage + overflow 错误恢复（见 chat.rs / 决策 16）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[specta(type = Number)]
    pub context_window: Option<u64>,
    /// 系统人设（preamble）。可选，用户在设置页配置。
    ///
    /// Skill 系统首次引入 preamble 机制（决策 20）：系统人设 + Skill Tier 1
    /// （<available_skills> 块）拼接为完整 preamble，注入 AgentBuilder::preamble。
    /// 拼在系统人设后，不破坏 prefix cache（前缀不变，skill 段是后缀追加）。
    /// None 时仅用 skill 段（若无 skill 则无 preamble）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preamble: Option<String>,
    /// 自定义 HTTP Header（代理、OpenRouter X-Title、Anthropic anthropic_betas 等）。
    /// 键值对，构造 client 时合并进 `Client::builder().http_headers()`。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
    /// 深度思考配置。None 等同 Off。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<ReasoningLevel>,
}

fn default_input_types() -> Vec<InputType> {
    vec![InputType::Text]
}

impl ProviderConfig {
    /// 便捷构造（向后兼容：旧 openai_compatible helper）。
    pub fn openai_compatible(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::OpenAiCompatible,
            api_key: api_key.into(),
            base_url: None,
            model: model.into(),
            temperature: None,
            max_tokens: None,
            top_p: None,
            supports_developer_role: None,
            supports_reasoning_effort: None,
            input_types: default_input_types(),
            context_window: None,
            preamble: None,
            extra_headers: None,
            reasoning: None,
        }
    }

    /// 解析后的 reasoning level（None → Off）。
    pub fn reasoning_level(&self) -> ReasoningLevel {
        self.reasoning.unwrap_or_default()
    }

    /// 解析后的 supports_developer_role（None → 按 kind 默认）。
    /// OpenAI 官方支持 developer role；兼容端点默认不支持。
    pub fn effective_supports_developer_role(&self) -> bool {
        self.supports_developer_role.unwrap_or(match self.kind {
            ProviderKind::OpenAi => true,
            // 兼容端点（DeepSeek/Ollama/vLLM 等）默认不认 developer role
            _ if self.kind.is_openai_family() => false,
            // Anthropic/Gemini 走各自协议，无 developer role 概念
            _ => false,
        })
    }

    /// 解析后的 supports_reasoning_effort（None → 按 kind 默认）。
    /// OpenAI 官方 + DeepSeek 支持；其余兼容端点默认不支持。
    pub fn effective_supports_reasoning_effort(&self) -> bool {
        self.supports_reasoning_effort.unwrap_or(match self.kind {
            ProviderKind::OpenAi | ProviderKind::DeepSeek => true,
            _ => false,
        })
    }

    /// 是否支持图片输入。
    pub fn supports_image(&self) -> bool {
        self.input_types.iter().any(|t| matches!(t, InputType::Image))
    }
}

/// 把 ProviderConfig.reasoning_level 映射为 provider 请求体的 additional_params。
///
/// - OpenAI 官方 / DeepSeek（支持 reasoning_effort）：`{ "reasoning_effort": "low"|"medium"|"high" }`
/// - Anthropic：`{ "thinking": { "type": "enabled", "budget_tokens": 4096|8192|16384 } }`
/// - Gemini：`{ "generation_config": { "thinking_config": { "thinking_budget": 0|8192|24576 } } }`
/// - 不支持 reasoning 的 provider / Off：返回 None
///
/// 注：rig 0.41 未在 agent builder 暴露 reasoning 选项，`additional_params`
/// 是官方留的 provider 透传口子（见 rig-agent builder.rs:274）。
pub fn reasoning_to_params(
    kind: ProviderKind,
    level: ReasoningLevel,
    supports_reasoning_effort: bool,
) -> Option<serde_json::Value> {
    if !level.enabled() {
        return None;
    }
    let effort = match level {
        ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::Off => return None,
    };
    match kind {
        // OpenAI 协议族：仅在 provider 支持 reasoning_effort 时发送
        _ if kind.is_openai_family() && supports_reasoning_effort => Some(serde_json::json!({
            "reasoning_effort": effort
        })),
        // OpenAI 协议族但不支持 reasoning_effort：drop（避免 400）
        _ if kind.is_openai_family() && !matches!(kind, ProviderKind::Zhipu) => None,
        // 智谱 GLM：OpenAI 兼容协议，用顶层 thinking 字段（非 reasoning_effort）
        ProviderKind::Zhipu => Some(serde_json::json!({
            "thinking": { "type": "enabled" }
        })),
        ProviderKind::Anthropic => {
            // budget_tokens 按级别映射（Claude extended thinking）
            let budget = match level {
                ReasoningLevel::Low => 4096u64,
                ReasoningLevel::Medium => 8192,
                ReasoningLevel::High => 16384,
                ReasoningLevel::Off => return None,
            };
            Some(serde_json::json!({
                "thinking": { "type": "enabled", "budget_tokens": budget }
            }))
        }
        ProviderKind::Gemini => {
            // Gemini thinking_config（0 = 关闭；这里 Off 已 return，故只映射预算）
            let budget = match level {
                ReasoningLevel::Low => 8192u64,
                ReasoningLevel::Medium => 16384,
                ReasoningLevel::High => 24576,
                ReasoningLevel::Off => return None,
            };
            Some(serde_json::json!({
                "generation_config": { "thinking_config": { "thinking_budget": budget } }
            }))
        }
        // 其余非 OpenAI 协议族 provider（理论上不会走到这，因 is_openai_family 已挡）
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parse_roundtrip() {
        for k in [
            ProviderKind::OpenAi,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::DeepSeek,
            ProviderKind::Xai,
            ProviderKind::Groq,
            ProviderKind::OpenRouter,
            ProviderKind::Ollama,
            ProviderKind::Moonshot,
            ProviderKind::Mistral,
            ProviderKind::Cohere,
            ProviderKind::Perplexity,
            ProviderKind::OpenAiCompatible,
        ] {
            assert_eq!(ProviderKind::parse(k.as_str()), Some(k), "roundtrip {k:?}");
        }
    }

    #[test]
    fn legacy_openai_compatible_value_maps_to_compat_kind() {
        // 一期旧配置值 "openai_compatible" 应映射到 OpenAiCompatible（兜底）
        assert_eq!(
            ProviderKind::parse("openai_compatible"),
            Some(ProviderKind::OpenAiCompatible)
        );
    }

    #[test]
    fn reasoning_off_returns_none() {
        assert_eq!(
            reasoning_to_params(ProviderKind::OpenAi, ReasoningLevel::Off, true),
            None
        );
        assert_eq!(
            reasoning_to_params(ProviderKind::Anthropic, ReasoningLevel::Off, true),
            None
        );
    }

    #[test]
    fn reasoning_openai_emits_effort() {
        let p = reasoning_to_params(ProviderKind::OpenAi, ReasoningLevel::High, true).unwrap();
        assert_eq!(p["reasoning_effort"], "high");
    }

    #[test]
    fn reasoning_dropped_when_unsupported() {
        // 兼容端点不支持 reasoning_effort → drop（None）
        assert_eq!(
            reasoning_to_params(ProviderKind::Ollama, ReasoningLevel::High, false),
            None
        );
    }

    #[test]
    fn reasoning_anthropic_emits_thinking() {
        let p = reasoning_to_params(ProviderKind::Anthropic, ReasoningLevel::Medium, true).unwrap();
        assert_eq!(p["thinking"]["type"], "enabled");
        assert_eq!(p["thinking"]["budget_tokens"], 8192);
    }

    #[test]
    fn reasoning_gemini_emits_thinking_config() {
        let p = reasoning_to_params(ProviderKind::Gemini, ReasoningLevel::High, true).unwrap();
        assert_eq!(p["generation_config"]["thinking_config"]["thinking_budget"], 24576);
    }

    #[test]
    fn effective_compatibility_defaults() {
        let mut cfg = ProviderConfig::openai_compatible("k", "m");
        cfg.kind = ProviderKind::OpenAi;
        assert!(cfg.effective_supports_developer_role());
        assert!(cfg.effective_supports_reasoning_effort());

        cfg.kind = ProviderKind::Ollama;
        assert!(!cfg.effective_supports_developer_role());
        assert!(!cfg.effective_supports_reasoning_effort());

        cfg.kind = ProviderKind::DeepSeek;
        assert!(!cfg.effective_supports_developer_role()); // 兼容端点
        assert!(cfg.effective_supports_reasoning_effort()); // DeepSeek 支持
    }

    #[test]
    fn explicit_compat_overrides_default() {
        let mut cfg = ProviderConfig::openai_compatible("k", "m");
        cfg.kind = ProviderKind::Ollama;
        cfg.supports_developer_role = Some(true); // 用户显式声明支持
        assert!(cfg.effective_supports_developer_role());
    }

    #[test]
    fn supports_image_detection() {
        let mut cfg = ProviderConfig::openai_compatible("k", "m");
        assert!(!cfg.supports_image());
        cfg.input_types = vec![InputType::Text, InputType::Image];
        assert!(cfg.supports_image());
    }
}
