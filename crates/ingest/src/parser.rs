//! DocumentParser trait（决策 8）+ 统一入口 `ingest_file`。
//!
//! trait 隔离底层库：换库只需改 parser 实现，上层（dispatcher / IPC）不动。
//!
//! 进度报告：有能力的 parser（pdf/epub/zip …）覆写 `parse_with_progress`，
//! 逐页/逐章/逐条目回调 `ParseProgress`；默认实现回退到无进度的 `parse`。

use crate::document::{Document, DocumentMeta};
use crate::error::IngestResult;
use std::path::Path;
use std::sync::Arc;

/// 解析进度回调。
///
/// 实现者通常把进度转发到 IPC Channel（如 Tauri `Channel<IngestProgress>`）。
/// 所有方法默认空实现，parser 按需调用。
///
/// **取消**：`is_cancelled()` 返回 true 时，parser 应尽快 break 循环并返回
/// `Err(IngestError::Cancelled)`。这是 cooperative cancellation——
/// 强制 abort（如 tokio::task::JoinHandle::abort）对底层库内部循环无效
/// （见 kreuzberg issue #789），只有 parser 主动检查才能可靠中止。
pub trait ParseProgress: Send + Sync {
    /// 进入新阶段（如 "打开文件" / "提取文本" / "解压条目"）。
    fn on_phase(&self, _phase: &str) {}

    /// 报告细粒度进度。
    ///
    /// - `current`：已处理单元数（页/章/条目）
    /// - `total`：总单元数；未知时为 `None`（前端显示 "current/?" 不确定态）
    fn on_progress(&self, _current: usize, _total: Option<usize>) {}

    /// 报告已产出的文本字符数（增量），用于大文件早期可见性。
    fn on_chars(&self, _delta: usize) {}

    /// 是否已取消。parser 在循环边界检查；true 时 break 并返回 Cancelled。
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 空实现，用于不需要进度的调用路径。
pub struct NoProgress;
impl ParseProgress for NoProgress {}

/// 文档解析器统一接口。
pub trait DocumentParser: Send + Sync {
    /// 该 parser 处理的格式名（用于 meta.format）。
    fn format_name(&self) -> &'static str;

    /// 解析给定路径的文件为 Document（无进度回调）。
    ///
    /// 默认实现：用 `NoProgress` 调 `parse_with_progress`。
    /// parser 只需实现 `parse_with_progress`（或两者都实现以保留快速路径）。
    fn parse(&self, path: &Path) -> IngestResult<Document> {
        self.parse_with_progress(path, &NoProgress)
    }

    /// 带进度回调的解析。
    ///
    /// 默认实现忽略 progress 直接调 `parse`（向后兼容）；
    /// 有能力的 parser 覆写此方法报告细粒度进度。
    fn parse_with_progress(&self, path: &Path, _progress: &dyn ParseProgress) -> IngestResult<Document> {
        self.parse(path)
    }
}

/// 顶层入口（无进度）：按扩展名/MIME 路由到具体 parser，返回 Document。
///
/// 路径不必存在校验（各 parser 自行处理）；大文件由 `check_size` 拦截。
pub fn ingest_file(path: &Path) -> IngestResult<Document> {
    crate::security::check_size(path)?;
    let parser = crate::dispatcher::pick_parser(path)?;
    parser.parse(path)
}

/// 顶层入口（带进度）：同 `ingest_file`，但把进度回调透传给 parser。
///
/// 上层（IPC 命令）传入一个 `ParseProgress` 实现，即可收到页/章/条目级进度，
/// 无需固定超时——前端通过进度流即可判断"还活着"还是"卡死"。
pub fn ingest_file_with_progress(
    path: &Path,
    progress: &dyn ParseProgress,
) -> IngestResult<Document> {
    tracing::info!(?path, "ingest: check_size");
    crate::security::check_size(path)?;
    progress.on_phase("校验文件");
    tracing::info!(?path, "ingest: pick_parser");
    let parser = crate::dispatcher::pick_parser(path)?;
    tracing::info!(?path, format = parser.format_name(), "ingest: parse_with_progress");
    parser.parse_with_progress(path, progress)
}

/// 构造元信息（各 parser 复用）。
pub(crate) fn make_meta(path: &Path, format: &str, source_bytes: u64) -> DocumentMeta {
    DocumentMeta {
        source_path: path.to_string_lossy().into_owned(),
        format: format.to_string(),
        char_count: 0,
        source_bytes,
    }
}

/// 便捷别名：Arc 包裹的进度回调，便于跨线程（spawn_blocking）传递。
pub type ProgressRef = Arc<dyn ParseProgress>;
