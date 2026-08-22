//! 联邦查询工具（三期阶段 1c：把数据源查询能力暴露给 agent）。
//!
//! 三个工具挂到 agent，让模型按需查询已注册数据源（见 PHASE3-FEDERATION.md §3.1）：
//!   - `list_data_sources`：列出已注册数据源 + 每源表清单 + 连接状态
//!   - `describe_table`：某数据源某表的列结构 + 前 5 行样本
//!   - `execute_sql`：执行只读 SELECT/WITH，返回 JSON 行集（含只读护栏 + 行数上限 + 超时）
//!
//! 与 `document_tools` 同构（闭包捕获 `Arc<FederationService>`），走同一条
//! `ToolServerHandle.add_dynamic_tool()` 注入路径（§2.1）。agent 自主决定调哪个工具、
//! 是否调——不强制注入到 system prompt（rig agent 工具自治范式）。
//!
//! 安全护栏（§3.2 三层防御）由 `federation::query::execute_query` 内部实现：
//! sqlparser 只读校验 + 行数硬上限（默认 200，最大 1000）+ 30s 超时。本工具层只做参数透传。

use std::sync::Arc;

use rig::tool::{DynamicTool, ToolOutput};
use serde_json::json;

use federation::FederationService;

/// 构造三个联邦查询工具，挂到 agent。共用同一个 `Arc<FederationService>`。
/// `allowed_sources` 限定本会话激活的数据源名集合——仅这些源对 agent 可见。
pub fn federation_tools(
    svc: Arc<FederationService>,
    allowed_sources: Arc<std::collections::HashSet<String>>,
) -> Vec<DynamicTool> {
    vec![
        list_data_sources_tool(svc.clone(), allowed_sources.clone()),
        describe_table_tool(svc.clone(), allowed_sources.clone()),
        execute_sql_tool(svc, allowed_sources),
    ]
}

/// `list_data_sources()`：列出已注册数据源 + 每源表清单 + 连接状态。
fn list_data_sources_tool(svc: Arc<FederationService>, allowed: Arc<std::collections::HashSet<String>>) -> DynamicTool {
    DynamicTool::new(
        "list_data_sources",
        "列出所有已注册的数据源（数据库/CSV/Excel）。返回每个数据源的 name（即 catalog 名，\
         SQL 三段式寻址前缀，如 `mydb.public.users`）、类型、连接状态、表数量。\
         当用户提到「数据库」「数据源」「表」或需要查结构化数据时，先调此工具了解有哪些源可用，\
         再用 describe_table 看具体表结构或 execute_sql 执行查询。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        move |_ctx, _args| {
            let svc = svc.clone();
            let allowed = allowed.clone();
            Box::pin(async move {
                match svc.list_sources().await {
                    Ok(sources) => {
                        // 仅返回本会话激活的数据源。
                        let sources: Vec<_> = sources.into_iter().filter(|s| allowed.contains(&s.name)).collect();
                        // 对每个已连接源附上表清单（agent 写 SQL 需要 catalog 名 + 表名）
                        let mut items = Vec::with_capacity(sources.len());
                        for s in sources {
                            let tables = if s.connected {
                                svc.browse_schema(&s.name)
                                    .await
                                    .map(|snap| {
                                        snap.tables.into_iter().map(|t| t.name).collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            items.push(json!({
                                "name": s.name,
                                "kind": s.kind.as_str(),
                                "connected": s.connected,
                                "table_count": s.table_count,
                                "tables": tables,
                                "last_error": s.last_error,
                            }));
                        }
                        let out = json!({ "data_sources": items, "count": items.len() });
                        Ok(ToolOutput::json(out))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：列出数据源失败 - {e}"))),
                }
            })
        },
    )
}

/// `describe_table(source_name, table_name)`：表结构 + 前 5 行样本。
fn describe_table_tool(svc: Arc<FederationService>, allowed: Arc<std::collections::HashSet<String>>) -> DynamicTool {
    DynamicTool::new(
        "describe_table",
        "查看某个数据源中某张表的结构（列名/类型/是否可空）+ 前 5 行样本数据。\
         先用 list_data_sources 拿到 source_name（catalog 名）和可用表名，\
         再用此工具了解列结构，最后用 execute_sql 写 SELECT 查询。\
         返回的列类型是 Arrow 类型名（Utf8=文本, Int64=整数, Float64=浮点, Date32=日期, Timestamp=时间戳）。",
        json!({
            "type": "object",
            "properties": {
                "source_name": {
                    "type": "string",
                    "description": "数据源名称（catalog 名，来自 list_data_sources）",
                },
                "table_name": {
                    "type": "string",
                    "description": "表名（来自 list_data_sources 的 tables 列表）",
                },
            },
            "required": ["source_name", "table_name"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let svc = svc.clone();
            let allowed = allowed.clone();
            Box::pin(async move {
                let source_name = args
                    .get("source_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let table_name = args
                    .get("table_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if source_name.is_empty() || table_name.is_empty() {
                    return Ok(ToolOutput::text("错误：source_name 和 table_name 不能为空"));
                }
                // 激活集过滤：该数据源不在本会话激活范围内则拒绝。
                if !allowed.contains(&source_name) {
                    return Ok(ToolOutput::text(format!(
                        "错误：数据源 `{source_name}` 不在当前会话激活范围内。请在会话范围中勾选该数据源。"
                    )));
                }
                match svc.browse_schema(&source_name).await {
                    Ok(snap) => {
                        // 找到目标表
                        let table = snap.tables.into_iter().find(|t| t.name == table_name);
                        match table {
                            Some(t) => {
                                let columns: Vec<_> = t
                                    .columns
                                    .into_iter()
                                    .map(|c| {
                                        json!({
                                            "name": c.name,
                                            "data_type": c.data_type,
                                            "nullable": c.nullable,
                                        })
                                    })
                                    .collect();
                                let out = json!({
                                    "source_name": source_name,
                                    "table_name": table_name,
                                    "columns": columns,
                                    "column_count": columns.len(),
                                    "row_count_estimate": t.row_count_estimate,
                                    "sample_rows": t.sample_rows,
                                });
                                Ok(ToolOutput::json(out))
                            }
                            None => Ok(ToolOutput::text(format!(
                                "错误：数据源 `{source_name}` 中找不到表 `{table_name}`"
                            ))),
                        }
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：查看表结构失败 - {e}"))),
                }
            })
        },
    )
}

/// `execute_sql(sql, limit?)`：执行只读 SELECT，返回 JSON 行集。
fn execute_sql_tool(svc: Arc<FederationService>, allowed: Arc<std::collections::HashSet<String>>) -> DynamicTool {
    DynamicTool::new(
        "execute_sql",
        "在已注册的数据源上执行只读 SQL 查询（SELECT / WITH，含跨源 JOIN）。\
         SQL 中表名用三段式 catalog 限定：`source_name.schema.table`（如 `mydb.public.users`），\
         source_name 来自 list_data_sources。跨源 JOIN 直接写两段三段式表名即可。\
         只读模式——INSERT/UPDATE/DELETE/DROP 等写操作会被拦截。\
         limit 控制返回行数（默认 200，最大 1000），未含 LIMIT 的 SQL 会自动追加。\
         返回 columns（列名/类型）+ rows（JSON 数组）+ row_count + elapsed_ms + sources_touched（涉及的源）。",
        json!({
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "只读 SQL（SELECT 或 WITH 语句）。表名用三段式 catalog.schema.table 限定。",
                },
                "limit": {
                    "type": "integer",
                    "description": "返回行数上限（默认 200，最大 1000）",
                    "default": 200,
                },
            },
            "required": ["sql"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let svc = svc.clone();
            let allowed = allowed.clone();
            Box::pin(async move {
                let sql = args
                    .get("sql")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.clamp(1, 1000) as usize);
                if sql.trim().is_empty() {
                    return Ok(ToolOutput::text("错误：sql 不能为空"));
                }
                // 激活集过滤：SQL 中引用的 catalog 名必须在激活范围内。
                // 简单检查：SQL 文本是否包含任一激活的 source_name。
                // （execute_query 内部还有只读护栏 + 行数上限 + 超时，这里只做激活集闸门。）
                let sql_lower = sql.to_lowercase();
                let touched: Vec<String> = allowed.iter().filter(|s| sql_lower.contains(&s.to_lowercase())).cloned().collect();
                if touched.is_empty() {
                    return Ok(ToolOutput::text(
                        "错误：SQL 未引用任何当前会话激活的数据源。请先用 list_data_sources 查看可用源，并在 SQL 中用三段式 `source_name.schema.table` 限定表名。"
                    ));
                }
                let ctx = svc.ctx().clone();
                match federation::query::execute_query(&ctx, &sql, limit).await {
                    Ok(result) => {
                        let columns: Vec<_> = result
                            .columns
                            .into_iter()
                            .map(|c| {
                                json!({
                                    "name": c.name,
                                    "data_type": c.data_type,
                                    "nullable": c.nullable,
                                })
                            })
                            .collect();
                        let out = json!({
                            "columns": columns,
                            "rows": result.rows,
                            "row_count": result.row_count,
                            "elapsed_ms": result.elapsed_ms,
                            "sources_touched": result.sources_touched,
                        });
                        Ok(ToolOutput::json(out))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：执行 SQL 失败 - {e}"))),
                }
            })
        },
    )
}
