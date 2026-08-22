//! 文件检索工具（一期收尾：agentic search 替代向量 RAG）。
//!
//! 三个工具挂到 agent，让模型按需检索知识库文档（而非自动注入切片）：
//!   - `list_documents`：列出已入库文档清单（不含全文）
//!   - `search_documents`：FTS5 关键词搜索（jieba 分词 + BM25 排序 + snippet 高亮）
//!   - `read_document`：按 id 分页读全文（offset/limit 按字符数）
//!
//! 设计理由（见 ARCHITECTURE.md 一期收尾决策）：
//!   - query 来自模型的工具调用（非用户原话），模型天然把口语化意图转成搜索关键词，
//!     FTS5 关键词匹配完美适配，无需向量检索的语义泛化
//!   - 业界共识（Anthropic Claude Code）：小语料文件工具优于 RAG
//!   - 符合原则 2（无本地模型）+ 原则 5（外部服务最小化：无需 embed API）
//!
//! 工具用 rig 的 DynamicTool（callback 闭包捕获 Arc<Memory>），挂 AgentBuilder.dynamic_tool()。

use std::sync::Arc;

use rig::tool::{DynamicTool, ToolOutput};
use serde_json::json;

use memory::Memory;

/// 构造三个文件检索工具，挂到 agent。共用同一个 `Arc<Memory>`。
pub fn document_tools(memory: Arc<Memory>, allowed_paths: Arc<std::collections::HashSet<String>>) -> Vec<DynamicTool> {
    vec![
        list_documents_tool(memory.clone(), allowed_paths.clone()),
        search_documents_tool(memory.clone(), allowed_paths.clone()),
        read_document_tool(memory, allowed_paths),
    ]
}

/// `list_documents()`：列出已入库文档（id/name/format/char_count）。
fn list_documents_tool(memory: Arc<Memory>, allowed_paths: Arc<std::collections::HashSet<String>>) -> DynamicTool {
    DynamicTool::new(
        "list_documents",
        "列出知识库中所有已入库的文档。返回每篇文档的 id、文件名、格式、字符数。\
         当用户提到「我上传的文件」「知识库里有什么」「之前那个文档」等，先调用此工具了解可用文档，\
         再用 search_documents 搜索或 read_document 读取。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        move |_ctx, _args| {
            let mem = memory.clone();
            let allowed = allowed_paths.clone();
            Box::pin(async move {
                match mem.list_documents() {
                    Ok(docs) => {
                        let items: Vec<_> = docs
                            .into_iter()
                            .filter(|(_, path, _, _, _, _, _)| allowed.contains(path))
                            .map(|(id, path, name, format, char_count, created_at, folder_path)| {
                                json!({
                                    "id": id,
                                    "name": name,
                                    "path": path,
                                    "format": format,
                                    "char_count": char_count,
                                    "created_at": created_at,
                                    "folder": folder_path,
                                })
                            })
                            .collect();
                        let out = json!({ "documents": items, "count": items.len() });
                        Ok(ToolOutput::json(out))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：列出文档失败 - {e}"))),
                }
            })
        },
    )
}

/// `search_documents(query, limit?)`：FTS5 关键词搜索。
fn search_documents_tool(memory: Arc<Memory>, allowed_paths: Arc<std::collections::HashSet<String>>) -> DynamicTool {
    DynamicTool::new(
        "search_documents",
        "在知识库文档中全文搜索。输入搜索关键词（中文词语，如「向量数据库」「知识图谱」），\
         返回匹配文档的 id、文件名、格式、高亮摘要片段、相关度排名。\
         多个词用空格分隔（AND 语义）。关键词应来自用户问题的核心概念，\
         不要直接传用户的整句口语化问题——提取关键名词再搜。\
         limit 默认 5，最多 20。",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词（中文词语，多个用空格分隔）",
                },
                "limit": {
                    "type": "integer",
                    "description": "返回结果数上限（默认 5，最大 20）",
                    "default": 5,
                },
            },
            "required": ["query"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let mem = memory.clone();
            let allowed = allowed_paths.clone();
            Box::pin(async move {
                let query = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.clamp(1, 20) as usize)
                    .unwrap_or(5);
                if query.trim().is_empty() {
                    return Ok(ToolOutput::text("错误：query 不能为空"));
                }
                match mem.search_documents(&query, limit) {
                    Ok(hits) => {
                        let items: Vec<_> = hits
                            .into_iter()
                            .filter(|h| allowed.contains(&h.path))
                            .map(|h| {
                                json!({
                                    "id": h.id,
                                    "name": h.name,
                                    "path": h.path,
                                    "snippet": h.snippet,
                                    "rank": h.rank,
                                })
                            })
                            .collect();
                        let out = json!({ "results": items, "count": items.len() });
                        Ok(ToolOutput::json(out))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：搜索失败 - {e}"))),
                }
            })
        },
    )
}

/// `read_document(id, offset?, limit?)`：按 id 分页读全文。
fn read_document_tool(memory: Arc<Memory>, allowed_paths: Arc<std::collections::HashSet<String>>) -> DynamicTool {
    DynamicTool::new(
        "read_document",
        "按 id 读取文档全文（可分页）。先用 list_documents 或 search_documents 拿到文档 id，\
         再用此工具读全文内容。offset/limit 按字符数（非字节），用于读取长文档的指定部分。\
         不传 offset/limit 返回全文。建议先读前 2000 字符了解内容，按需翻页。",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "文档 id（来自 list_documents 或 search_documents）",
                },
                "offset": {
                    "type": "integer",
                    "description": "起始字符位置（默认 0）",
                    "default": 0,
                },
                "limit": {
                    "type": "integer",
                    "description": "读取字符数上限（默认返回全文）",
                },
            },
            "required": ["id"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let mem = memory.clone();
            let allowed = allowed_paths.clone();
            Box::pin(async move {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let offset = args
                    .get("offset")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.max(0) as usize);
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.max(0) as usize);
                if id.is_empty() {
                    return Ok(ToolOutput::text("错误：id 不能为空"));
                }
                match mem.read_document(&id, offset, limit) {
                    Ok(Some((path, name, format, text, char_count))) => {
                        // 激活集过滤：该文档不在本会话激活范围内则拒绝（避免模型读未挂载的文件）。
                        if !allowed.contains(&path) {
                            return Ok(ToolOutput::text(format!(
                                "错误：文档 {name} 不在当前会话激活的知识范围内。请先在会话范围中挂载该文件所在的文件夹，或用 @文件名 引用它。"
                            )));
                        }
                        let out = json!({
                            "id": id,
                            "name": name,
                            "path": path,
                            "format": format,
                            "char_count": char_count,
                            "offset": offset.unwrap_or(0),
                            "text": text,
                        });
                        Ok(ToolOutput::json(out))
                    }
                    Ok(None) => Ok(ToolOutput::text(format!("错误：找不到 id 为 {id} 的文档"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：读取文档失败 - {e}"))),
                }
            })
        },
    )
}
