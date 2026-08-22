//! ingest IPC 命令（§14.2 文件摄入流）。
//!
//! `ingest_files(paths, on_progress)`：batch 级并行摄取，进度经 Channel 回推。
//! 每个文件：queued → parsing（逐页/章/条目细粒度进度）→ done/error/cancelled。
//!
//! 机制（对齐开源实践：kreuzberg / Tauri+git2 / RAGFlow）：
//! - **batch 级并行**：多文件用 `JoinSet` + 信号量限并发（默认 num_cpus×1.5）
//! - **单文件串行**：parser 内部逐页/章串行，保逐页进度 + 可取消
//! - **cooperative 取消**：parser 循环检查 `is_cancelled()`，true 时 break 返回 Cancelled
//! - **IPC 节流**：60ms 时间窗，避免高频进度事件卡死前端（git2+Tauri 实践）
//! - **无固定超时**：超时对底层库内部循环无效（kreuzberg issue #789），靠进度+取消

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::ipc::Channel;
use tauri::State;

use crate::commands::error::{AppError, AppResult};
use crate::state::AppState;
use specta_typescript::Number;

/// 单个摄取任务的进度事件（前端 IngestStatusBoard 据此更新卡片）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IngestProgress {
    pub job_id: String,
    pub path: String,
    pub file_name: String,
    pub stage: IngestStage,
    /// 已产出字符数（parsing 阶段渐进更新）。
    pub char_count: u32,
    /// error 阶段的错误信息。
    pub error: Option<String>,
    /// 当前解析阶段描述（如 "提取 PDF 文本"）。仅在 parsing 阶段有意义。
    #[serde(default)]
    pub phase: Option<String>,
    /// 细粒度进度：已处理单元数（页/章/条目）。仅在 parsing 阶段渐进更新。
    #[serde(default)]
    pub current: Option<u32>,
    /// 细粒度进度：总单元数；未知时为 None（前端显示 "current/?"）。
    #[serde(default)]
    pub total: Option<u32>,
    /// 文件大小（字节）。Queued 时即上报，供前端在文件名旁展示。
    #[serde(default)]
    #[specta(type = Number)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub enum IngestStage {
    Queued,
    Parsing,
    Done,
    Error,
    /// 用户取消
    Cancelled,
}

/// 摄取完成后的产物摘要（前端据此展示/挂载到会话）。
/// 不含全文——全文由 ingest_files 内部直接 upsert 到 documents 表，前端按需
/// read_document(id) 取全文（决策 17：避免大全文经 IPC 往返，触发 payload 限制）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IngestResultItem {
    pub job_id: String,
    pub path: String,
    pub file_name: String,
    pub format: String,
    pub char_count: u32,
    /// 多模态 part 数（图片输入）。
    pub multimodal_count: u32,
    /// 文件大小（字节）。
    #[serde(default)]
    #[specta(type = Number)]
    pub file_size: Option<u64>,
}

// ── 取消注册表（Tauri State） ──────────────────────────────────────────

/// 全局取消注册表：job_id → cancel flag。
/// `ingest_files` 为每个文件注册一个 `Arc<AtomicBool>`；
/// `cancel_ingest` 设置对应 flag 为 true，parser 循环检查后 cooperative 退出。
#[derive(Default)]
pub struct CancelRegistry {
    flags: Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
}

impl CancelRegistry {
    fn register(&self, job_id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.flags
            .lock()
            .expect("CancelRegistry poisoned")
            .insert(job_id.to_string(), flag.clone());
        flag
    }

    fn cancel(&self, job_id: &str) -> bool {
        if let Some(flag) = self.flags.lock().expect("poisoned").get(job_id) {
            flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn remove(&self, job_id: &str) {
        self.flags.lock().expect("poisoned").remove(job_id);
    }
}

/// 取消指定摄取任务（cooperative）。
///
/// parser 在下一次循环边界检查到 cancel flag 后 break，返回 Cancelled。
/// 对底层库内部阻塞（如 lopdf::Document::load）无法立即中止——只在循环点生效。
#[tauri::command]
#[specta::specta]
pub async fn cancel_ingest(job_id: String, registry: State<'_, CancelRegistry>) -> AppResult<bool> {
    let found = registry.cancel(&job_id);
    if found {
        tracing::info!(%job_id, "ingest: cancel requested");
    }
    Ok(found)
}

/// batch 级最大并发数（kreuzberg 默认 num_cpus×1.5）。
fn max_concurrent() -> usize {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    ((n as f64) * 1.5).ceil() as usize
}

/// 摄取一批文件（batch 级并行）。
///
/// 流程（§14.2）：
/// 1. 为每个 path 生成 job_id，注册 cancel flag，发 Queued
/// 2. 用 `JoinSet` + 信号量限并发，每个文件 `spawn_blocking` 串行解析
/// 3. 各文件独立发 Parsing 进度（经节流桥接）→ Done/Error/Cancelled
///
/// 单文件内串行（保逐页进度 + 可取消），多文件并行（限并发数）。
#[tauri::command]
#[specta::specta]
pub async fn ingest_files(
    paths: Vec<String>,
    on_progress: Channel<IngestProgress>,
    // 来源会话 ID。会话内上传时传（文件落 /Inbox + 记 source_conv_id + 自动加入该会话激活集）。
    // Library 上传时传 None（文件落到调用方指定的 folder，不自动激活——由调用方保证）。
    conversation_id: Option<String>,
    // Library 上传时的目标文件夹（仅 conversation_id=None 时生效；会话上传仍落 /Inbox）。
    // None = 根目录散文件。非 None 时后端 normalize_folder_path 保证 '/' 前缀。
    folder_path: Option<String>,
    registry: State<'_, CancelRegistry>,
    state: State<'_, AppState>,
) -> AppResult<Vec<IngestResultItem>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    // 准备每个文件的元数据 + cancel flag
    let specs: Vec<JobSpec> = paths
        .into_iter()
        .enumerate()
        .map(|(i, raw)| {
            let path = PathBuf::from(&raw);
            let job_id = format!("ingest-{i}-{}", uuid::Uuid::new_v4());
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&raw)
                .to_string();
            let cancel = registry.register(&job_id);
            // 读文件大小（供前端在文件名旁展示）。读不到不阻断流程。
            let file_size = std::fs::metadata(&path).ok().map(|m| m.len());
            JobSpec { job_id, path, raw, file_name, cancel, file_size }
        })
        .collect();

    // 先发所有 Queued（前端立即看到全部排队文件）
    for s in &specs {
        let _ = on_progress.send(s.progress(IngestStage::Queued, 0, None, None, None, None));
    }

    // batch 级并行：信号量限并发，JoinSet 收集
    let sem = Arc::new(tokio::sync::Semaphore::new(max_concurrent()));
    let mut join_set = tokio::task::JoinSet::new();

    for s in specs {
        let permit_sem = sem.clone();
        let channel = on_progress.clone();
        join_set.spawn(async move {
            // 等待并发许可（batch 级排队）
            let _permit = permit_sem.acquire().await.expect("sem closed");

            let _ = channel.send(s.progress(
                IngestStage::Parsing, 0, None,
                Some("准备解析".into()), None, None,
            ));

            tracing::info!(path = ?s.path, "ingest: start parse");
            let bridge = ChannelProgressBridge {
                channel: channel.clone(),
                job_id: s.job_id.clone(),
                path: s.raw.clone(),
                file_name: s.file_name.clone(),
                file_size: s.file_size,
                char_count: AtomicU32::new(0),
                current_unit: AtomicU32::new(0),
                total_units: AtomicU32::new(0),
                cancel: s.cancel.clone(),
                last_send: Mutex::new(Instant::now() - Duration::from_secs(1)),
            };
            let progress: ingest::ProgressRef = Arc::new(bridge);
            let path = s.path.clone();
            let result = tokio::task::spawn_blocking({
                let progress = progress.clone();
                move || ingest::ingest_file_with_progress(&path, progress.as_ref())
            })
            .await;
            tracing::info!(path = ?s.path, "ingest: parse done");
            (s, result)
        });
    }

    // 收集结果（完成顺序不固定，但 IngestResultItem 自带 job_id 可对应）
    let mut results = Vec::with_capacity(join_set.len());
    while let Some(joined) = join_set.join_next().await {
        let (s, result) = joined.expect("ingest task panicked");
        registry.remove(&s.job_id);

        match result {
            Ok(Ok(doc)) => {
                let char_count = doc.meta.char_count as u32;
                let format = doc.meta.format.clone();
                let multimodal_count = doc.multimodal_parts.len() as u32;
                let _ = on_progress.send(s.progress(
                    IngestStage::Done, char_count, None, None, None, None,
                ));
                // 成功文档立即持久化全文到 documents 表（供 search/read 工具检索）。
                // 同 path upsert 幂等。空文本跳过（parser 理论上不会返回空，防御）。
                //
                // **两阶段**：
                // 1. upsert 只写主行（毫秒级），spawn_blocking + await 保证 mountDocument
                //    时主行已入库（mountDocument 靠 path 查 documents 表）。
                // 2. FTS5 索引构建 fire-and-forget（不 await）：jieba 对大全文分词耗时
                //    数分钟，异步在独立连接后台建索引，不阻塞 ingest_files 返回、
                //    不抢主连接锁。期间该文档搜不到（indexed_at=0），索引完成后可搜。
                if !doc.text.is_empty() {
                    let memory = state.memory.clone();
                    let raw = s.raw.clone();
                    let row = memory::DocumentRow {
                        id: memory::new_document_id(),
                        path: memory::canonicalize_path(&s.raw),
                        name: s.file_name.clone(),
                        format: format.clone(),
                        text: doc.text,
                        char_count,
                        created_at: memory::now_ms(),
                        // 会话上传 → 落 /Inbox + 记来源会话；
                        // Library 上传 → 用调用方传入的 folder_path（normalize 保证 '/' 前缀），None=根目录散文件
                        folder_path: if conversation_id.is_some() {
                            Some("/Inbox".to_string())
                        } else {
                            folder_path.as_deref().map(memory::normalize_folder_path).filter(|p| !p.is_empty())
                        },
                        source_conv_id: conversation_id.clone(),
                    };
                    // 阶段1：upsert 主行（await，保证后续 mountDocument 能查到）。
                    let doc_id = match tokio::task::spawn_blocking({
                        let memory = memory.clone();
                        move || memory.upsert_document(row)
                    }).await {
                        Ok(Ok(id)) => {
                            tracing::info!(path = %raw, "ingest: document persisted, scheduling FTS index");
                            id
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(path = %raw, error = %e, "ingest: persist document failed");
                            // upsert 失败则跳过索引。
                            String::new()
                        }
                        Err(join_err) => {
                            tracing::error!(path = %raw, error = %join_err, "ingest: persist task panicked");
                            String::new()
                        }
                    };
                    // 阶段2：异步建 FTS5 索引（fire-and-forget，不 await）。
                    // 在独立连接上执行，不抢主连接 Mutex 锁。
                    if !doc_id.is_empty() {
                        let mem = memory.clone();
                        let raw2 = raw.clone();
                        tokio::task::spawn_blocking(move || {
                            match mem.index_document(&doc_id) {
                                Ok(()) => tracing::info!(path = %raw2, "ingest: FTS index built"),
                                Err(e) => tracing::warn!(path = %raw2, error = %e, "ingest: FTS index build failed"),
                            }
                        });
                    }
                    // 会话上传 → 自动加入该会话激活集 documents 部分（mount_document 幂等）。
                    // 用户上传后立即可 @引用 / 工具检索，不必手动挂载。
                    if let Some(conv_id) = &conversation_id {
                        let mem = memory.clone();
                        let raw_path = raw.clone();
                        let conv = conv_id.clone();
                        if let Err(e) = tokio::task::spawn_blocking(move || {
                            mem.mount_document(&conv, &memory::canonicalize_path(&raw_path))
                        }).await {
                            tracing::warn!(path = %raw, error = %e, "ingest: auto-mount failed");
                        }
                    }
                }
                results.push(IngestResultItem {
                    job_id: s.job_id,
                    path: s.raw,
                    file_name: s.file_name,
                    format,
                    char_count,
                    multimodal_count,
                    file_size: s.file_size,
                });
            }
            Ok(Err(ingest::IngestError::Cancelled)) => {
                tracing::info!(path = ?s.path, "ingest: cancelled");
                let _ = on_progress.send(s.progress(
                    IngestStage::Cancelled, 0, Some("已取消".into()), None, None, None,
                ));
                results.push(IngestResultItem {
                    job_id: s.job_id,
                    path: s.raw,
                    file_name: s.file_name,
                    format: "cancelled".into(),
                    char_count: 0,
                    multimodal_count: 0,
                    file_size: s.file_size,
                });
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                tracing::warn!(path = ?s.path, err = %msg, "ingest: parse failed");
                let _ = on_progress.send(s.progress(
                    IngestStage::Error, 0, Some(msg.clone()), None, None, None,
                ));
                results.push(IngestResultItem {
                    job_id: s.job_id,
                    path: s.raw,
                    file_name: s.file_name,
                    format: "error".into(),
                    char_count: 0,
                    multimodal_count: 0,
                    file_size: s.file_size,
                });
            }
            Err(join_err) => {
                let msg = format!("解析任务异常退出: {join_err}");
                tracing::error!(path = ?s.path, %msg);
                let _ = on_progress.send(s.progress(
                    IngestStage::Error, 0, Some(msg.clone()), None, None, None,
                ));
                results.push(IngestResultItem {
                    job_id: s.job_id,
                    path: s.raw,
                    file_name: s.file_name,
                    format: "error".into(),
                    char_count: 0,
                    multimodal_count: 0,
                    file_size: s.file_size,
                });
            }
        }
    }

    // 持久化已移到上方 Ok(Ok(doc)) 分支（成功文档立即 upsert，避免持有全量 text
    // 再二次循环）。此处仅返回摘要结果给前端（不含全文，见 IngestResultItem 文档）。

    Ok(results)
}

/// 单个摄取任务规格（注册 cancel flag + 构造 progress 事件）。
struct JobSpec {
    job_id: String,
    path: PathBuf,
    raw: String,
    file_name: String,
    cancel: Arc<AtomicBool>,
    /// 文件大小（字节）。读不到时为 None（如路径无效）。
    file_size: Option<u64>,
}

impl JobSpec {
    #[allow(clippy::too_many_arguments)]
    fn progress(
        &self,
        stage: IngestStage,
        char_count: u32,
        error: Option<String>,
        phase: Option<String>,
        current: Option<u32>,
        total: Option<u32>,
    ) -> IngestProgress {
        IngestProgress {
            job_id: self.job_id.clone(),
            path: self.raw.clone(),
            file_name: self.file_name.clone(),
            stage,
            char_count,
            error,
            phase,
            current,
            total,
            file_size: self.file_size,
        }
    }
}

// ── ParseProgress → Channel 桥接（带 60ms 节流 + cooperative 取消） ──────

/// 把 ingest crate 的 `ParseProgress` 回调桥接到 Tauri IPC `Channel<IngestProgress>`。
///
/// **节流**：`on_progress` / `on_chars` 距上次发送 < 60ms 则跳过（参考 git2+Tauri
/// 实践，高频 IPC 会卡死前端）。`on_phase` 总是发送（阶段切换低频且重要）。
/// **取消**：`is_cancelled()` 读 `cancel: AtomicBool`，由 `cancel_ingest` 命令设置。
struct ChannelProgressBridge {
    channel: Channel<IngestProgress>,
    job_id: String,
    path: String,
    file_name: String,
    /// 文件大小（字节），随每个进度事件透传给前端。
    file_size: Option<u64>,
    /// 累计已产出字符数（原子，parser 跨线程调用安全）。
    char_count: AtomicU32,
    /// 已处理单元数（页/章/条目）。0 表示尚未上报（前端映射为 None）。
    /// 缓存而非丢弃，确保节流跳过后下次任意 emit 都能带出最新页进度。
    current_unit: AtomicU32,
    /// 总单元数。0 表示尚未上报（未知，前端显示非确定态）。
    total_units: AtomicU32,
    /// cooperative 取消标志（由 CancelRegistry 设置）。
    cancel: Arc<AtomicBool>,
    /// 上次 IPC 发送时间（60ms 节流）。
    last_send: Mutex<Instant>,
}

/// 节流窗口：两次进度 IPC 最小间隔。
const THROTTLE: Duration = Duration::from_millis(60);

impl ChannelProgressBridge {
    /// 是否通过了节流窗口（可发送）。同时更新 last_send。
    fn should_send(&self) -> bool {
        let mut last = self.last_send.lock().expect("poisoned");
        if last.elapsed() >= THROTTLE {
            *last = Instant::now();
            true
        } else {
            false
        }
    }

    fn emit(&self, stage: IngestStage, char_count: u32, error: Option<String>,
            phase: Option<String>, current: Option<u32>, total: Option<u32>) {
        // current/total 传入 None 时，回退到缓存的最新值（on_progress 节流跳过时
        // 仍保留进度，由后续 on_chars/on_phase 的 emit 带出，避免百分比永远收不到）。
        let cur = current.or_else(|| {
            let v = self.current_unit.load(Ordering::Relaxed);
            (v > 0).then_some(v)
        });
        let tot = total.or_else(|| {
            let v = self.total_units.load(Ordering::Relaxed);
            (v > 0).then_some(v)
        });
        let _ = self.channel.send(IngestProgress {
            job_id: self.job_id.clone(),
            path: self.path.clone(),
            file_name: self.file_name.clone(),
            stage,
            char_count,
            error,
            phase,
            current: cur,
            total: tot,
            file_size: self.file_size,
        });
    }
}

impl ingest::ParseProgress for ChannelProgressBridge {
    fn on_phase(&self, phase: &str) {
        // phase 低频且重要，不节流
        let cur = self.char_count.load(Ordering::Relaxed);
        self.emit(IngestStage::Parsing, cur, None, Some(phase.to_string()), None, None);
    }

    fn on_progress(&self, current: usize, total: Option<usize>) {
        // 缓存最新单元进度（不节流），确保后续任意 emit 都能带出百分比
        self.current_unit.store(current as u32, Ordering::Relaxed);
        if let Some(t) = total {
            self.total_units.store(t as u32, Ordering::Relaxed);
        }
        if self.is_cancelled() || !self.should_send() {
            return;
        }
        let cur = self.char_count.load(Ordering::Relaxed);
        self.emit(
            IngestStage::Parsing, cur, None, None,
            Some(current as u32), total.map(|t| t as u32),
        );
    }

    fn on_chars(&self, delta: usize) {
        let cur = self.char_count.fetch_add(delta as u32, Ordering::Relaxed) + delta as u32;
        if !self.should_send() {
            return;
        }
        self.emit(IngestStage::Parsing, cur, None, None, None, None);
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// 文件夹树节点（后端构建层级，前端直接渲染）。
///
/// 文件夹由文件的 `folder_path` 隐式定义；后端从 DISTINCT folder_path
/// 构建嵌套树，Inbox 置顶，其余按名排序。把树构建放后端是因为层级解析 +
/// 排序是领域逻辑，不应散在前端。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct FolderNodeDto {
    /// 当前层名字（不含路径前缀，如 "书信集"）。
    pub name: String,
    /// 完整路径（如 "/曾国藩专题/书信集"）。
    pub path: String,
    /// 子文件夹（已排序，Inbox 置顶）。
    pub children: Vec<FolderNodeDto>,
}

/// 从 `memory::FolderNode` 递归转换为 DTO。
fn folder_node_to_dto(node: &memory::FolderNode) -> FolderNodeDto {
    FolderNodeDto {
        name: node.name.clone(),
        path: node.path.clone(),
        children: node.children.iter().map(folder_node_to_dto).collect(),
    }
}

/// 会话已挂载文档的摘要（不含全文，前端 Inspector 展示用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MountedDocDto {
    pub path: String,
    pub name: String,
    pub format: String,
    pub char_count: u32,
    #[specta(type = Number)]
    pub mounted_at: i64,
}

/// 知识库全部入库文档的摘要（不含全文，`@` 菜单选择用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DocumentSummaryDto {
    pub id: String,
    pub path: String,
    pub name: String,
    pub format: String,
    pub char_count: u32,
    #[specta(type = Number)]
    pub created_at: i64,
    pub folder_path: Option<String>,
}

/// 挂载文档到会话（`@` 挂载持久化）。幂等。
/// path 必须先经 ingest 摄取（documents 表已有该 path）。
#[tauri::command]
#[specta::specta]
pub async fn mount_document(
    conversation_id: String,
    path: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let path = memory::canonicalize_path(&path);
    // 校验 documents 表存在该 path（不存在则拒绝挂载）。
    match state.memory.document_id_by_path(&path) {
        Ok(Some(_)) => {
            state.memory.mount_document(&conversation_id, &path)?;
            Ok(true)
        }
        Ok(None) => Ok(false), // 文档未入库，前端提示重新摄取
        Err(e) => Err(AppError::Memory(e.to_string())),
    }
}

/// 卸载会话下的某篇文档（不删 documents 全文）。
#[tauri::command]
#[specta::specta]
pub async fn unmount_document(
    conversation_id: String,
    path: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let path = memory::canonicalize_path(&path);
    let n = state.memory.unmount_document(&conversation_id, &path)?;
    Ok(n > 0)
}

/// 列出会话已挂载的文档（按挂载时间排序，不含全文）。
#[tauri::command]
#[specta::specta]
pub async fn list_mounted_documents(
    conversation_id: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<MountedDocDto>> {
    let docs = state.memory.list_mounted_documents(&conversation_id)?;
    Ok(docs
        .into_iter()
        .map(|d| MountedDocDto {
            path: d.path,
            name: d.name,
            format: d.format,
            char_count: d.char_count,
            mounted_at: d.mounted_at,
        })
        .collect())
}

/// 列出知识库全部入库文档（不含全文，`@` 菜单选择挂载用）。
#[tauri::command]
#[specta::specta]
pub async fn list_all_documents(
    state: State<'_, AppState>,
) -> AppResult<Vec<DocumentSummaryDto>> {
    let docs = state.memory.list_documents()?;
    Ok(docs
        .into_iter()
        .map(|(id, path, name, format, char_count, created_at, folder_path)| DocumentSummaryDto {
            id,
            path,
            name,
            format,
            char_count,
            created_at,
            folder_path,
        })
        .collect())
}

/// 按 path 删除知识库文档（同时清 FTS5 索引 + 所有会话的挂载关联）。
#[tauri::command]
#[specta::specta]
pub async fn delete_document(
    path: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let path = memory::canonicalize_path(&path);
    // 先清所有会话对该文档的挂载关联（conversation_documents 无 FK CASCADE）。
    let _ = state.memory.clear_mounted_documents_by_path(&path);
    let n = state.memory.delete_document_by_path(&path)?;
    Ok(n > 0)
}

/// 按 id 读取文档全文（可分页，预览用）。
#[tauri::command]
#[specta::specta]
pub async fn read_document(
    id: String,
    offset: Option<u32>,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> AppResult<Option<DocumentContentDto>> {
    Ok(state.memory.read_document(&id, offset.map(|n| n as usize), limit.map(|n| n as usize))?.map(
        |(path, name, format, text, char_count)| DocumentContentDto {
            path,
            name,
            format,
            text,
            char_count,
        },
    ))
}

/// 文档全文内容（预览抽屉用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DocumentContentDto {
    pub path: String,
    pub name: String,
    pub format: String,
    pub text: String,
    pub char_count: u32,
}

// ───────── 文件夹操作（CONVERSATION-SCOPE.md §6.1）─────────
// 独立 folders 表 + documents.folder_path 双轨（决策 19 修订）：
// folders 表持久化空文件夹；documents.folder_path 隐式推导有文件的文件夹。
// 文件夹树 = 两者 UNION 去重。

/// 新建空文件夹（持久化到 folders 表）。
/// path 如 "/曾国藩专题" 或 "/曾国藩专题/书信集"。自动推导 parent_path/name。
/// 已存在则忽略（幂等）。返回是否实际创建。
#[tauri::command]
#[specta::specta]
pub async fn create_folder(
    path: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    Ok(state.memory.create_folder(&path)?)
}

/// 列出文件夹树（后端已构建嵌套层级 + 排序，前端直接渲染）。
///
/// 替代旧版扁平 `list_folders` 返回 `Vec<String>` 的做法——
/// 树构建是领域逻辑，不应在前端解析 `/` 拆分。
#[tauri::command]
#[specta::specta]
pub async fn list_folders(state: State<'_, AppState>) -> AppResult<Vec<FolderNodeDto>> {
    let tree = state.memory.list_folder_tree()?;
    Ok(tree.iter().map(folder_node_to_dto).collect())
}

/// 列出指定文件夹下的直接子文件（Library 右栏展示）。
/// folder = null 或 "/" 表示根目录散文件（folder_path IS NULL）。
#[tauri::command]
#[specta::specta]
pub async fn list_documents_by_folder(
    folder: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<DocumentSummaryDto>> {
    Ok(state
        .memory
        .list_documents_by_folder(folder.as_deref())?
        .into_iter()
        .map(|(id, path, name, format, char_count, created_at)| DocumentSummaryDto {
            id,
            path,
            name,
            format,
            char_count,
            created_at,
            folder_path: folder.clone(),
        })
        .collect())
}

/// 移动单个文件到目标文件夹。target_folder=null 表示根目录散文件。
#[tauri::command]
#[specta::specta]
pub async fn move_document(
    path: String,
    target_folder: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let path = memory::canonicalize_path(&path);
    let n = state.memory.move_document(&path, target_folder.as_deref())?;
    Ok(n > 0)
}

/// 重命名文件夹（递归处理子文件夹）。
/// old_path 如 "/曾国藩专题", new_path 如 "/曾公研究"。
#[tauri::command]
#[specta::specta]
pub async fn rename_folder(
    old_path: String,
    new_path: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let n = state.memory.rename_folder(&old_path, &new_path)?;
    Ok(n > 0)
}

/// 删除文件夹及其下所有文件（含子文件夹递归）。
/// 每个文件走 delete_document_by_path（清 documents 行 + FTS5 索引 + conversation_documents 关联）。
/// 返回删除的文件数。
#[tauri::command]
#[specta::specta]
pub async fn delete_folder(
    folder: String,
    state: State<'_, AppState>,
) -> AppResult<u32> {
    Ok(state.memory.delete_folder(&folder)? as u32)
}

// ───────── 会话激活集（CONVERSATION-SCOPE.md §2.2）─────────

/// 会话激活集快照（供前端 chip 展示 + 工具过滤）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ActiveScopeDto {
    /// 激活的文件夹路径（含子目录递归）。
    pub folders: Vec<String>,
    /// 激活的单文件 path（@触发，conversation_documents 表）。
    pub documents: Vec<String>,
    /// 激活的数据源名。
    pub sources: Vec<String>,
    /// 激活的本体 api_name（@OntologyName 引用，如 ["SupplyChain"]）。
    pub ontologies: Vec<String>,
}

/// 读取会话激活集。
#[tauri::command]
#[specta::specta]
pub async fn get_active_scope(
    conversation_id: String,
    state: State<'_, AppState>,
) -> AppResult<ActiveScopeDto> {
    let folders = state.memory.get_active_folders(&conversation_id)?;
    let sources = state.memory.get_active_sources(&conversation_id)?;
    let ontologies = state.memory.get_active_ontologies(&conversation_id)?;
    // documents 部分来自 conversation_documents 表（复用现有 list_mounted_documents）。
    let documents = state
        .memory
        .list_mounted_documents(&conversation_id)?
        .into_iter()
        .map(|d| d.path)
        .collect();
    Ok(ActiveScopeDto {
        folders,
        documents,
        sources,
        ontologies,
    })
}

/// 设置会话激活集的文件夹部分。
#[tauri::command]
#[specta::specta]
pub async fn set_active_folders(
    conversation_id: String,
    folders: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    state.memory.set_active_folders(&conversation_id, &folders)?;
    Ok(())
}

/// 设置会话激活集的数据源部分。
#[tauri::command]
#[specta::specta]
pub async fn set_active_sources(
    conversation_id: String,
    sources: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    state.memory.set_active_sources(&conversation_id, &sources)?;
    Ok(())
}

/// 设置会话激活集的本体部分（@OntologyName 引用本体）。
///
/// 存 ontology api_name 列表。传入空 Vec 清空引用。
/// 会话模式挂 5 个只读 drill-in 工具，agent 按 api_name 钻取 schema（不注入全文）。
#[tauri::command]
#[specta::specta]
pub async fn set_active_ontologies(
    conversation_id: String,
    ontologies: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    state.memory.set_active_ontologies(&conversation_id, &ontologies)?;
    Ok(())
}
