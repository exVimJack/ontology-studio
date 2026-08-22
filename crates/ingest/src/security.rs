//! 安全防护：文件大小上限、zip 炸弹防护、解压深度/总数限制（§六 security.rs）。
//!
//! 纯防御性检查，避免恶意/畸形文件耗尽内存。

use crate::error::{IngestError, IngestResult};
use std::path::Path;

/// 单文件大小上限（一期 200MB；超大文档二期走流式分块）。
pub const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;

/// 压缩包解压后总展开字节上限（防 zip 炸弹）。
pub const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 500 * 1024 * 1024;

/// 压缩包内条目数上限。
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// 压缩包递归深度上限（防嵌套压缩包 DoS）。
pub const MAX_ARCHIVE_DEPTH: usize = 5;

/// 解压单条目大小上限（流式读取时累计校验）。
pub const MAX_ARCHIVE_ENTRY_BYTES: u64 = 100 * 1024 * 1024;

/// 解压速率异常阈值（压缩比超过此值告警，但仍按总量上限硬截断）。
pub const COMPRESSION_RATIO_WARN: u64 = 100;

/// 校验源文件大小。
pub fn check_size(path: &Path) -> IngestResult<()> {
    let meta = std::fs::metadata(path)
        .map_err(|e| IngestError::Io(std::io::Error::other(format!("读取文件元信息失败: {e}"))))?;
    let size = meta.len();
    if size > MAX_FILE_BYTES {
        return Err(IngestError::FileTooLarge {
            actual_bytes: size,
            limit_bytes: MAX_FILE_BYTES,
        });
    }
    Ok(())
}

/// 流式读取压缩包条目时的累计计数器（防 zip 炸弹）。
pub struct ArchiveBudget {
    pub total_expanded: u64,
    pub entries: usize,
}

impl Default for ArchiveBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveBudget {
    pub fn new() -> Self {
        Self {
            total_expanded: 0,
            entries: 0,
        }
    }

    /// 记录一条目已展开字节数；超限返回 SecurityViolation。
    pub fn account(&mut self, entry_bytes: u64) -> IngestResult<()> {
        if entry_bytes > MAX_ARCHIVE_ENTRY_BYTES {
            return Err(IngestError::SecurityViolation(format!(
                "单条目过大: {entry_bytes} 字节（上限 {MAX_ARCHIVE_ENTRY_BYTES}）"
            )));
        }
        self.total_expanded = self.total_expanded.saturating_add(entry_bytes);
        self.entries += 1;
        if self.total_expanded > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(IngestError::SecurityViolation(format!(
                "解压总量超限: {0} 字节（上限 {MAX_ARCHIVE_EXPANDED_BYTES}）",
                self.total_expanded
            )));
        }
        if self.entries > MAX_ARCHIVE_ENTRIES {
            return Err(IngestError::SecurityViolation(format!(
                "条目数超限: {0}（上限 {MAX_ARCHIVE_ENTRIES}）",
                self.entries
            )));
        }
        Ok(())
    }
}
