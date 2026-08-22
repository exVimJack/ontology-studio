//! 模型上下文窗口解析（二期 B1 配套）。
//!
//! 设计（见 ARCHITECTURE.md 决策 16 / 调研记录）：
//! 五层 fallback 获取模型上下文窗口（token 数）：
//!   1. **用户显式配置**：`ProviderConfig.context_window`（设置页可填，preset 预填）。
//!      用户最清楚自己用的模型变体，优先级最高。
//!   2. **官方元数据探测**：按 provider kind 分派探测端点，统一解析官方窗口字段。
//!      OpenAI 兼容族（Kimi/OpenRouter/DeepSeek 等）`GET {base_url}/models` 解析
//!      `context_length`；Anthropic `GET /v1/models` 解析 `context_window`；
//!      Gemini `GET /v1beta/models/{model}` 解析 `inputTokenLimit`。
//!      **不针对特定 provider 写死**，新模型自动支持，零维护。
//!      结果带 TTL 缓存（base_url+model 为 key），配置变更时 `invalidate_cache()` 清空。
//!      （注：OpenAI 官方 / DeepSeek / GLM 等不暴露窗口字段，探测自然降级，不报错。）
//!   3. **Ollama 特例**：`POST /api/show` 读 GGUF 元数据 `llama.context_length`
//!      （Ollama 用原生 API 而非 OpenAI 兼容端点暴露窗口）。
//!   4. **内置已知模型表**：探测拿不到（DeepSeek 等 `/models` 不暴露 `context_length`）
//!      时，按（provider kind + 模型名前缀）查内置知识库，取官方文档窗口
//!      （如 DeepSeek V4 系列 1M）。纯静态数据、同步可查，启动期 restore 也能用。
//!   5. **保守默认**：100K（覆盖多数主流模型，偏松安全侧）。
//!
//! **探测是"锦上添花"，绝不影响配置和对话主流程**：
//!   - 整个探测用 `tokio::time::timeout(PROBE_TIMEOUT, ...)` 包住，超时即降级
//!   - 任何错误（网络/解析/状态码/panic）都静默返回 None
//!   - 探测失败 → 用户配置 → 默认，对话照常进行
//!
//! context_window 只是「提前预防」的判据；真正的安全网是运行时的
//! **真实 usage + overflow 错误恢复**（见 chat.rs / 决策 16），即使
//! context_window 完全未知，provider 报 context 超限时仍能压缩重试。

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use tokio::time::timeout;

use crate::provider::{ProviderConfig, ProviderKind};

/// 探测超时（秒）。探测是辅助，不能拖慢对话首字延迟。
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 缓存 TTL。模型窗口不常变，1 小时足够。
const CACHE_TTL: Duration = Duration::from_secs(3600);

/// 保守默认窗口（最终 fallback）。
pub const DEFAULT_CONTEXT_WINDOW: usize = 100_000;

/// 缓存条目。
struct CacheEntry {
    window: usize,
    fetched_at: Instant,
}

/// 进程级缓存：provider+model → 窗口。OnceLock 懒初始化。
static CACHE: OnceLock<std::sync::Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static std::sync::Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 解析模型上下文窗口（五层 fallback）。
///
/// 优先级：用户配置 → 缓存 → 通用探测 → 内置已知模型表 → 默认。
/// 探测全程超时+错误保护，任何异常都静默降级，绝不返回 Err。
pub async fn resolve_context_window(config: &ProviderConfig) -> usize {
    // 1. 用户显式配置（最高优先级；<=0 视为无效输入，降级自动探测）
    if let Some(w) = config.context_window.filter(|w| *w > 0) {
        return w as usize;
    }

    let key = cache_key(config);

    // 2. 查缓存
    {
        let guard = cache().lock().unwrap();
        if let Some(entry) = guard.get(&key) {
            if entry.fetched_at.elapsed() < CACHE_TTL {
                return entry.window;
            }
        }
    }

    // 3. 通用探测（超时+错误保护，失败降级）
    let probed = probe_context_window(config).await;

    // 4. 探测失败 → 内置已知模型表（DeepSeek 等 /models 不暴露 context_length 的 provider）
    // 5. 表未命中 → 保守默认
    let window = probed
        .or_else(|| known_model_window(config))
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);

    // 写缓存（探测失败的默认值也缓存，避免反复打网络）
    {
        let mut guard = cache().lock().unwrap();
        guard.insert(
            key,
            CacheEntry {
                window,
                fetched_at: Instant::now(),
            },
        );
    }
    window
}

/// 同步清缓存（provider 配置变更时调用）。
pub fn invalidate_cache() {
    let mut guard = cache().lock().unwrap();
    guard.clear();
}

// ── 内置已知模型表（探测失败的知识库兜底） ────────────────────────

/// 内置已知模型上下文窗口条目。
///
/// 覆盖 `/v1/models` 不暴露 `context_length` 的 provider（DeepSeek/GLM/MiniMax 等），
/// 探测失败时的知识库兜底。数值取官方公开文档；模型演进后增补条目即可，
/// 不需要改探测逻辑。匹配规则：provider kind 命中 + 模型名前缀命中（支持变体）。
struct KnownModel {
    /// 适用 provider kind。Ollama 不在表内（走 /api/show 探测，本地量化模型窗口不可猜）。
    kinds: &'static [ProviderKind],
    /// 模型名前缀（大小写敏感，模型名通常小写）。
    prefix: &'static str,
    /// 官方上下文窗口（token 数）。
    window: usize,
}

/// 已知模型表（按 model 名前缀匹配）。
///
/// DeepSeek V4 系列（deepseek-v4-pro / deepseek-v4-flash，官方 1M 上下文）；
/// 旧版 deepseek-chat / deepseek-reasoner（V3.2，官方 128K）。
/// 这些模型 `/models` 端点不返回 `context_length`，探测必失败，靠本表兜底。
static KNOWN_MODELS: &[KnownModel] = &[
    KnownModel {
        kinds: &[ProviderKind::DeepSeek, ProviderKind::OpenAiCompatible],
        prefix: "deepseek-v4",
        window: 1_000_000,
    },
    KnownModel {
        kinds: &[ProviderKind::DeepSeek, ProviderKind::OpenAiCompatible],
        prefix: "deepseek-chat",
        window: 128_000,
    },
    KnownModel {
        kinds: &[ProviderKind::DeepSeek, ProviderKind::OpenAiCompatible],
        prefix: "deepseek-reasoner",
        window: 128_000,
    },
];

/// 内置已知模型表匹配（纯静态，无网络）。
///
/// kind 命中 + 模型名前缀命中 → 官方窗口；否则 None（上层降级保守默认）。
fn known_model_window(config: &ProviderConfig) -> Option<usize> {
    KNOWN_MODELS
        .iter()
        .find(|m| m.kinds.contains(&config.kind) && config.model.starts_with(m.prefix))
        .map(|m| m.window)
}

/// 同步解析（无网络）：用户配置 → 内置已知模型表 → 保守默认。
///
/// 供启动期 `restore_provider` 用（启动时无法 async 探测 /v1/models）。
/// DeepSeek V4 等表内模型启动即拿到官方窗口，无需等用户重设 provider。
pub fn resolve_known_or_default(config: &ProviderConfig) -> usize {
    config
        .context_window
        .filter(|w| *w > 0)
        .map(|w| w as usize)
        .or_else(|| known_model_window(config))
        .unwrap_or(DEFAULT_CONTEXT_WINDOW)
}

/// 缓存键：kind + base_url + model。
fn cache_key(config: &ProviderConfig) -> String {
    format!(
        "{:?}|{}|{}",
        config.kind,
        config.base_url.as_deref().unwrap_or(""),
        config.model
    )
}

// ── 运行时探测（全程超时+错误保护） ────────────────────────────

/// 尝试运行时探测模型窗口。成功返回 Some，失败/超时/任何异常返回 None。
///
/// 整个探测用 `tokio::time::timeout` 包住，内部每一步都用 `.ok()?` 吞错。
/// 按 provider kind 分派（业界标准做法：能拿官方元数据的尽量拿）：
///   - Ollama（kind=ollama 或 base_url 含 11434）→ `POST /api/show` 读 GGUF 元数据
///   - Anthropic → `GET /v1/models` 读 `context_window`
///   - Gemini → `GET /v1beta/models/{model}` 读 `inputTokenLimit`
///   - 其余（OpenAI 兼容族）→ `GET {base_url}/models` 宽容解析 context_length
async fn probe_context_window(config: &ProviderConfig) -> Option<usize> {
    // 整段超时包裹：任何慢/卡死都不拖慢对话
    match timeout(PROBE_TIMEOUT, probe_inner(config)).await {
        Ok(result) => result,
        Err(_elapsed) => {
            tracing::debug!(model = %config.model, "context_window probe timed out");
            None
        }
    }
}

async fn probe_inner(config: &ProviderConfig) -> Option<usize> {
    let base = config.base_url.as_deref().unwrap_or("");
    // Ollama 特例：kind=ollama，或 OpenAiCompatible 连了本地 11434（兼容端点当 Ollama 用）
    if config.kind == ProviderKind::Ollama || is_ollama(base) {
        return probe_ollama(base, &config.model).await;
    }
    match config.kind {
        ProviderKind::Anthropic => probe_anthropic(config).await,
        ProviderKind::Gemini => probe_gemini(config).await,
        _ => probe_openai_models(base, &config.model).await,
    }
}

fn is_ollama(base_url: &str) -> bool {
    (base_url.contains("localhost") || base_url.contains("127.0.0.1")) && base_url.contains("11434")
}

/// 通用 OpenAI 兼容探测：`GET {base_url}/models` → 找 model → 读 context_length。
///
/// 宽容解析，兼容多种响应结构：
///   - OpenAI 标准：`{ data: [{ id, ... }] }`（无 context_length，自然降级）
///   - Kimi/Moonshot：`{ data: [{ id, context_length, supports_* }] }`
///   - OpenRouter：`{ data: [{ id, context_length, top_provider: { context_length } }] }`
///   - 其他兼容端点可能把窗口塞在任意字段，用 serde_json::Value 通配查找
///
/// 匹配策略：精确 id → id 含 model → model 含 id。
async fn probe_openai_models(base_url: &str, model: &str) -> Option<usize> {
    if base_url.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // 用 Value 通配解析，避免对每种 provider 写死 struct
    let body: Value = resp.json().await.ok()?;
    let data = body.get("data").and_then(|d| d.as_array())?;
    let entry = find_model_entry(data, model)?;
    extract_context_length(entry)
}

/// Anthropic：`GET /v1/models` 读官方 `context_window` 字段。
///
/// Anthropic 的 model 列表端点原生返回窗口（`{ data: [{ id, display_name, context_window }] }`），
/// 不需要像 OpenAI 兼容端点那样猜字段。鉴权：`x-api-key` + `anthropic-version`。
/// base_url 为空用官方默认 `https://api.anthropic.com`。
async fn probe_anthropic(config: &ProviderConfig) -> Option<usize> {
    let base = config
        .base_url
        .as_deref()
        .unwrap_or("https://api.anthropic.com");
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    let url = format!("{}/v1/models", base.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    parse_anthropic_context_window(&body, &config.model)
}

/// 解析 Anthropic `/v1/models` 响应：找匹配模型读 `context_window`（纯函数，可单测）。
fn parse_anthropic_context_window(body: &Value, model: &str) -> Option<usize> {
    let data = body.get("data").and_then(|d| d.as_array())?;
    let entry = find_model_entry(data, model)?;
    entry
        .get("context_window")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
        .map(|v| v as usize)
}

/// Gemini：`GET /v1beta/models/{model}` 读官方 `inputTokenLimit` 字段。
///
/// Gemini 用单模型端点（models.get）而非列表端点（列表不返回窗口），
/// `inputTokenLimit` 即上下文窗口内可输入的最大 token。
/// 鉴权：`x-goog-api-key`。base_url 为空用官方默认。
async fn probe_gemini(config: &ProviderConfig) -> Option<usize> {
    let base = config
        .base_url
        .as_deref()
        .unwrap_or("https://generativelanguage.googleapis.com");
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    let url = format!(
        "{}/v1beta/models/{}",
        base.trim_end_matches('/'),
        config.model
    );
    let resp = client
        .get(&url)
        .header("x-goog-api-key", &config.api_key)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    parse_gemini_input_token_limit(&body)
}

/// 解析 Gemini `models.get` 响应：读 `inputTokenLimit`（纯函数，可单测）。
fn parse_gemini_input_token_limit(body: &Value) -> Option<usize> {
    body.get("inputTokenLimit")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
        .map(|v| v as usize)
}

/// 在模型列表里找匹配项：精确 id → id 含 model → model 含 id。
fn find_model_entry<'a>(data: &'a [Value], model: &str) -> Option<&'a Value> {
    data.iter()
        .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(model))
        .or_else(|| {
            data.iter().find(|m| {
                m.get("id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| id.contains(model))
            })
        })
        .or_else(|| {
            data.iter().find(|m| {
                m.get("id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| model.contains(id))
            })
        })
}

/// 从单个模型 JSON 对象里宽容提取 context_length。
///
/// 检查顺序（覆盖已知 provider 字段名）：
///   1. `context_length`（Kimi/OpenRouter/多数兼容端点）
///   2. `top_provider.context_length`（OpenRouter 嵌套）
///   3. `max_context_tokens` / `context_window`（少数变体）
fn extract_context_length(entry: &Value) -> Option<usize> {
    // 直接字段
    if let Some(v) = entry.get("context_length").and_then(|v| v.as_u64()) {
        if v > 0 {
            return Some(v as usize);
        }
    }
    // OpenRouter 嵌套
    if let Some(v) = entry
        .get("top_provider")
        .and_then(|t| t.get("context_length"))
        .and_then(|v| v.as_u64())
    {
        if v > 0 {
            return Some(v as usize);
        }
    }
    // 变体字段名
    for key in ["max_context_tokens", "context_window", "max_input_tokens"] {
        if let Some(v) = entry.get(key).and_then(|v| v.as_u64()) {
            if v > 0 {
                return Some(v as usize);
            }
        }
    }
    None
}

/// Ollama：`POST /api/show` body `{ model }` → 读 `model_info["llama.context_length"]`。
///
/// Ollama 的 OpenAI-compatible base_url 形如 `http://localhost:11434/v1`，
/// 原生 API 在 `/api/...`，需把 `/v1` 去掉拼 `/api/show`。
async fn probe_ollama(base_url: &str, model: &str) -> Option<usize> {
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    // base_url 形如 http://localhost:11434/v1 → http://localhost:11434
    let root = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{root}/api/show");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // model_info 是个 flat map，key 形如 "llama.context_length"，value 是字符串
    #[derive(Deserialize)]
    struct ShowResp {
        #[serde(default)]
        model_info: HashMap<String, String>,
    }
    let body: ShowResp = resp.json().await.ok()?;
    body.model_info
        .get("llama.context_length")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
}

// ── 测试 ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    fn cfg(kind: ProviderKind, base_url: Option<&str>, model: &str) -> ProviderConfig {
        ProviderConfig {
            kind,
            api_key: "sk-test".into(),
            base_url: base_url.map(String::from),
            model: model.into(),
            context_window: None,
            preamble: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            supports_developer_role: None,
            supports_reasoning_effort: None,
            input_types: vec![crate::provider::InputType::Text],
            extra_headers: None,
            reasoning: None,
        }
    }

    fn cfg_with_window(model: &str, window: u64) -> ProviderConfig {
        ProviderConfig {
            kind: ProviderKind::OpenAiCompatible,
            api_key: "sk-test".into(),
            base_url: Some("https://example.com/v1".into()),
            model: model.into(),
            context_window: Some(window),
            preamble: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            supports_developer_role: None,
            supports_reasoning_effort: None,
            input_types: vec![crate::provider::InputType::Text],
            extra_headers: None,
            reasoning: None,
        }
    }

    // ── extract_context_length 宽容解析 ──

    #[test]
    fn extract_direct_context_length() {
        let entry = serde_json::json!({ "id": "kimi-k3", "context_length": 262144 });
        assert_eq!(extract_context_length(&entry), Some(262144));
    }

    #[test]
    fn extract_openrouter_nested() {
        let entry = serde_json::json!({
            "id": "deepseek/deepseek-v4-flash",
            "context_length": 64000,
            "top_provider": { "context_length": 64000 }
        });
        assert_eq!(extract_context_length(&entry), Some(64000));
    }

    #[test]
    fn extract_variant_field_names() {
        let entry = serde_json::json!({ "id": "x", "max_context_tokens": 131072 });
        assert_eq!(extract_context_length(&entry), Some(131072));
        let entry = serde_json::json!({ "id": "x", "context_window": 200000 });
        assert_eq!(extract_context_length(&entry), Some(200000));
    }

    #[test]
    fn extract_zero_treated_as_missing() {
        // 0 是哨兵值，不当真值
        let entry = serde_json::json!({ "id": "x", "context_length": 0 });
        assert_eq!(extract_context_length(&entry), None);
    }

    #[test]
    fn extract_missing_returns_none() {
        // OpenAI 官方格式没有 context_length
        let entry = serde_json::json!({ "id": "gpt-4o", "object": "model", "owned_by": "openai" });
        assert_eq!(extract_context_length(&entry), None);
    }

    // ── Anthropic /v1/models（官方 context_window 字段） ──

    #[test]
    fn anthropic_parse_hits_context_window() {
        let body = serde_json::json!({
            "data": [
                { "id": "claude-opus-5", "display_name": "Claude Opus 5", "context_window": 1000000 },
                { "id": "claude-sonnet-5", "display_name": "Claude Sonnet 5", "context_window": 200000 }
            ],
            "has_more": false
        });
        assert_eq!(parse_anthropic_context_window(&body, "claude-sonnet-5"), Some(200_000));
    }

    #[test]
    fn anthropic_parse_model_not_found() {
        let body = serde_json::json!({
            "data": [
                { "id": "claude-opus-5", "display_name": "Claude Opus 5", "context_window": 1000000 }
            ],
            "has_more": false
        });
        assert_eq!(parse_anthropic_context_window(&body, "unknown-model"), None);
    }

    #[test]
    fn anthropic_parse_missing_window_field() {
        // 某些代理实现可能不返回 context_window → 降级
        let body = serde_json::json!({
            "data": [
                { "id": "claude-sonnet-5", "display_name": "Claude Sonnet 5" }
            ],
            "has_more": false
        });
        assert_eq!(parse_anthropic_context_window(&body, "claude-sonnet-5"), None);
    }

    // ── Gemini models.get（官方 inputTokenLimit 字段） ──

    #[test]
    fn gemini_parse_hits_input_token_limit() {
        let body = serde_json::json!({
            "name": "models/gemini-3.1-pro",
            "inputTokenLimit": 1048576,
            "outputTokenLimit": 65536
        });
        assert_eq!(parse_gemini_input_token_limit(&body), Some(1_048_576));
    }

    #[test]
    fn gemini_parse_missing_limit() {
        let body = serde_json::json!({ "name": "models/gemini-3.1-pro", "description": "no limits" });
        assert_eq!(parse_gemini_input_token_limit(&body), None);
    }

    // ── find_model_entry 匹配策略 ──

    #[test]
    fn find_entry_exact_then_contains_then_reverse() {
        let data = serde_json::json!([
            { "id": "deepseek-chat", "object": "model" },
            { "id": "deepseek/deepseek-reasoner", "object": "model" }
        ]);
        let arr = data.as_array().unwrap();
        // 精确命中
        assert_eq!(find_model_entry(arr, "deepseek-chat").unwrap()["id"], "deepseek-chat");
        // id 含 model（openrouter 风格 deepseek/xxx）
        assert_eq!(find_model_entry(arr, "deepseek-reasoner").unwrap()["id"], "deepseek/deepseek-reasoner");
        // 未命中
        assert_eq!(find_model_entry(arr, "gpt-4o"), None);
    }

    // ── is_ollama 检测 ──

    #[test]
    fn is_ollama_detection() {
        assert!(is_ollama("http://localhost:11434/v1"));
        assert!(is_ollama("http://127.0.0.1:11434/v1"));
        assert!(!is_ollama("https://api.openai.com/v1"));
        assert!(!is_ollama("https://openrouter.ai/api/v1"));
        assert!(!is_ollama(""));
    }

    // ── cache_key ──

    #[test]
    fn cache_key_differs_by_model() {
        let c1 = cfg(ProviderKind::OpenAiCompatible, None, "gpt-4o");
        let c2 = cfg(ProviderKind::OpenAiCompatible, None, "gpt-4o-mini");
        assert_ne!(cache_key(&c1), cache_key(&c2));
    }

    #[test]
    fn cache_key_differs_by_base_url() {
        let c1 = cfg(ProviderKind::OpenAiCompatible, Some("https://a.com/v1"), "m");
        let c2 = cfg(ProviderKind::OpenAiCompatible, Some("https://b.com/v1"), "m");
        assert_ne!(cache_key(&c1), cache_key(&c2));
    }

    // ── 内置已知模型表 ──

    #[test]
    fn known_model_deepseek_v4_series() {
        // 设置页预设 deepseek-v4-pro / deepseek-v4-flash（官方 1M）
        let c = cfg(ProviderKind::DeepSeek, None, "deepseek-v4-pro");
        assert_eq!(known_model_window(&c), Some(1_000_000));
        let c = cfg(ProviderKind::DeepSeek, None, "deepseek-v4-flash");
        assert_eq!(known_model_window(&c), Some(1_000_000));
    }

    #[test]
    fn known_model_legacy_deepseek() {
        let c = cfg(ProviderKind::DeepSeek, None, "deepseek-chat");
        assert_eq!(known_model_window(&c), Some(128_000));
        let c = cfg(ProviderKind::DeepSeek, None, "deepseek-reasoner");
        assert_eq!(known_model_window(&c), Some(128_000));
    }

    #[test]
    fn known_model_miss_returns_none() {
        // 未知模型 → None，走保守默认
        let c = cfg(ProviderKind::DeepSeek, None, "my-finetune");
        assert_eq!(known_model_window(&c), None);
        // 表内模型名 + 非表内 kind → None
        let c = cfg(ProviderKind::OpenAi, None, "deepseek-v4-pro");
        assert_eq!(known_model_window(&c), None);
        // Ollama 走 /api/show 探测，不进表（避免本地量化模型误命中 1M）
        let c = cfg(ProviderKind::Ollama, None, "deepseek-v4-flash:7b");
        assert_eq!(known_model_window(&c), None);
    }

    #[tokio::test]
    async fn resolve_uses_known_table_when_probe_fails() {
        // DeepSeek /models 不暴露 context_length：探测必然失败（不可达端点），
        // 应命中内置表（1M）而不是保守默认 100K
        invalidate_cache();
        let c = cfg(
            ProviderKind::DeepSeek,
            Some("http://127.0.0.1:39997/v1"),
            "deepseek-v4-pro",
        );
        let w = resolve_context_window(&c).await;
        assert_eq!(w, 1_000_000);
    }

    // ── resolve_known_or_default（同步，启动期 restore 用） ──

    #[test]
    fn resolve_known_or_default_prefers_user_config() {
        let c = cfg_with_window("deepseek-v4-pro", 42_000);
        assert_eq!(resolve_known_or_default(&c), 42_000);
    }

    #[test]
    fn resolve_known_or_default_ignores_zero_config() {
        // 0 是无效输入，降级到表/默认（负数在 serde→u64 阶段已被拒绝，到不了这里）
        let c = cfg_with_window("deepseek-v4-pro", 0);
        assert_eq!(resolve_known_or_default(&c), 1_000_000);
        let c = cfg_with_window("unknown-model", 0);
        assert_eq!(resolve_known_or_default(&c), DEFAULT_CONTEXT_WINDOW);
    }

    #[test]
    fn resolve_known_or_default_falls_back_to_table() {
        let c = cfg(ProviderKind::DeepSeek, None, "deepseek-v4-flash");
        assert_eq!(resolve_known_or_default(&c), 1_000_000);
    }

    #[test]
    fn resolve_known_or_default_defaults_when_unknown() {
        let c = cfg(ProviderKind::DeepSeek, None, "totally-unknown-model");
        assert_eq!(resolve_known_or_default(&c), DEFAULT_CONTEXT_WINDOW);
    }

    // ── resolve_context_window：用户配置优先 ──

    #[tokio::test]
    async fn resolve_user_config_takes_precedence() {
        // 用户填了 50000，即使 base_url 指向不存在的端点，也用 50000
        let c = cfg_with_window("any-model", 50_000);
        let w = resolve_context_window(&c).await;
        assert_eq!(w, 50_000);
    }

    #[tokio::test]
    async fn resolve_user_config_zero_treated_as_invalid() {
        // 填 0（或负数）是无效输入：降级自动探测/已知表，而不是把窗口设为 0
        invalidate_cache();
        // 命中已知表（deepseek-v4 → 1M）
        let c = cfg_with_window("deepseek-v4-pro", 0);
        let w = resolve_context_window(&c).await;
        assert_eq!(w, 1_000_000);
        // 未命中表 → 保守默认
        let c = cfg_with_window("unknown-model", 0);
        let w = resolve_context_window(&c).await;
        assert_eq!(w, DEFAULT_CONTEXT_WINDOW);
    }

    #[tokio::test]
    async fn resolve_falls_back_to_default_when_probe_fails() {
        // 指向不存在的本地端口，探测必然失败/超时，应降级到默认
        invalidate_cache();
        let c = cfg(
            ProviderKind::OpenAiCompatible,
            Some("http://127.0.0.1:39998/v1"),
            "unknown-model",
        );
        let w = timeout(Duration::from_secs(15), resolve_context_window(&c))
            .await
            .expect("resolve itself must not hang");
        assert_eq!(w, DEFAULT_CONTEXT_WINDOW);
    }

    #[tokio::test]
    async fn resolve_does_not_hang_on_unreachable() {
        // 整段探测有超时保护，resolve 不会卡死
        invalidate_cache();
        let c = cfg(
            ProviderKind::OpenAiCompatible,
            Some("http://10.255.255.1/v1"),
            "x",
        );
        let start = Instant::now();
        let w = timeout(Duration::from_secs(20), resolve_context_window(&c))
            .await
            .expect("resolve must complete within probe timeout");
        // 探测超时后应快速返回默认
        assert!(start.elapsed() < Duration::from_secs(15));
        assert_eq!(w, DEFAULT_CONTEXT_WINDOW);
    }
}
