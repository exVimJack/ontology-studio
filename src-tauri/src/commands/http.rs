//! 通用 HTTP POST 代理（Rust 侧 reqwest）。
//!
//! 动机（2026-09 宪章 AI 建议，docs/CHARTER-SOVEREIGNTY.md）：webview fetch
//! 直连 LLM API 受 CORS 预检限制——部分网关对 OPTIONS 返回 404（实测火山方舟
//! API 网关），预检失败即 "Failed to Fetch"。LLM 请求统一走本命令
//! （Rust reqwest 无 CORS 概念）。
//!
//! 安全边界：仅允许 https；仅面向前端 LLM 调用场景，不作任意代理。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::error::{AppError, AppResult};

/// POST 结果：状态码 + 响应体原文（非 2xx 由调用方按 LLM 错误格式处理）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HttpPostResult {
    pub status: u16,
    pub body: String,
}

#[tauri::command]
#[specta::specta]
pub async fn http_post_json(
    url: String,
    headers: HashMap<String, String>,
    body: String,
) -> AppResult<HttpPostResult> {
    if !url.starts_with("https://") {
        return Err(AppError::Provider("仅允许 https URL".into()));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| AppError::Provider(format!("构建 HTTP 客户端失败: {e}")))?;
    let mut req = client.post(&url).header("Content-Type", "application/json");
    for (k, v) in &headers {
        req = req.header(k, v);
    }
    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|e| AppError::Provider(format!("请求失败: {e}")))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| AppError::Provider(format!("读取响应失败: {e}")))?;
    Ok(HttpPostResult { status, body: text })
}
