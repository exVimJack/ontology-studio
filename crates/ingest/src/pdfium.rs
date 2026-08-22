//! PDFium 单例管理（决策 5）。
//!
//! `Pdfium`（FFI 绑定 + 动态库句柄）开销集中在 `bind_to_library` 一次性加载，
//! 反复 load/unload 动态库既慢又有风险。故用进程级 `OnceLock` 单例持有，
//! `PdfParser` 每次解析借用同一个 `Pdfium`。
//!
//! ## 线程安全（关键修正）
//! - PDFium C 库**完全非线程安全**。官方 `fpdfview.h` 明确：
//!   "None of the PDFium APIs are thread-safe. They expect to be called from a
//!    single thread. Barring that, embedders are required to ensure (via a mutex
//!    or similar) that only a single PDFium call can be made at a time."
//! - `pdfium-render` 的 `thread_safe` feature **仅**为 `Pdfium`/`PdfDocument` 等结构体
//!   impl `Send + Sync`（把 bindings 存全局 `OnceCell`），**并不提供任何调用级 mutex**
//!   串行化对 C 库的访问（见 pdfium-render issue #20「Marshall calls to Pdfium」、
//!   issue #66「sync is not safe in every situation」）。
//! - 因此多线程并发调 pdfium（即使操作不同文档）会导致内存损坏：实测并发 load
//!   3 份 PDF 触发 `STATUS_STACK_BUFFER_OVERRUN`；交错 load+extract 触发
//!   `PdfiumLibraryInternalError(FormatError)`——即用户报告的错误。
//! - 本 crate 用进程级 `Mutex` 串行化所有 pdfium 操作（load→extract→drop 整个临界区）。
//!   `PdfParser::parse_with_progress` 通过 [`with_pdfium`] 借用，持锁期间其他 pdfium
//!   调用排队等待。这是 PDFium 单线程约束的必然代价，多文件 batch 实际串行执行。
//!
//! ## 初始化
//! `src-tauri` 启动时（setup hook）定位打包的 PDFium 动态库，调 `init_pdfium(path)`。
//! 未初始化时 `PdfParser` 返回明确错误（不静默回退到系统库——避免开发/生产行为不一致）。

use crate::error::{IngestError, IngestResult};
use pdfium_render::prelude::Pdfium;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

/// 初始化串行化锁。
///
/// [`init_pdfium`] 里的 `Pdfium::bind_to_library` + `Pdfium::new` 会往 pdfium-render 的
/// 进程级全局 `BINDINGS` OnceLock 上 `set`，而 `Pdfium::new` 对重复 set 会 `assert!` panic。
/// 因此仅靠 [`PDFIUM`] 的 `get_or_init` 无法防并发——两个线程可能都在 `get_or_init`
/// 之前各自调了 `Pdfium::new` 触发第二次 assert。用此锁把整个 bind 过程串行化，
/// 并在持锁后重查单例，保证多线程（如测试）并发首次调用也只绑定一次。

/// 进程级 PDFium 调用串行化锁。
///
/// PDFium C 库非线程安全（见模块文档），所有对 pdfium 的调用（load/extract/render/drop）
/// 必须互斥。此锁在首次借用时惰性创建。`PdfParser` 在整个 load→extract→drop 临界区
/// 持有此锁，确保任意时刻只有一个线程在调用 PDFium。
static PDFIUM_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 借用进程级 PDFium 串行化锁的 guard。
///
/// 调用方应在整个 pdfium 操作临界区（从 `load_pdf_from_file` 到 `PdfDocument` drop）
/// 持有此 guard。drop guard 即释放锁，允许下一个排队线程进入。
///
/// 互斥本身不会死锁（单锁、不重入），但临界区内不得回调任何会再次借用此锁的代码。
/// `ParseProgress` 回调（IPC 节流/取消检查）不碰 pdfium，安全。
pub fn with_pdfium() -> IngestResult<std::sync::MutexGuard<'static, ()>> {
    let lock = PDFIUM_LOCK.get_or_init(|| Mutex::new(()));
    lock.lock().map_err(|e| {
        IngestError::Pdf(format!("PDFium 串行化锁 poisoned: {e}"))
    })
}

/// 初始化进程级 PDFium 单例：加载指定路径的动态库并绑定。
///
/// `lib_path` 指向**文件**（如 `pdfium.dll` / `libpdfium.dylib` / `libpdfium.so`）。
/// 由 `src-tauri` 在启动时调用一次；重复调用返回已有单例（忽略新路径）。
///
/// 失败原因：动态库文件不存在、架构不匹配、版本与 `pdfium_7881` feature 不一致
/// （后者表现为 `bind_to_library` 返回 missing-symbol 错误）。
pub fn init_pdfium(lib_path: &Path) -> IngestResult<()> {
    // 快路径：已初始化则直接返回，避免无谓加锁。
    if PDFIUM.get().is_some() {
        tracing::debug!(?lib_path, "pdfium: already initialized, skip");
        return Ok(());
    }
    // 串行化 bind + 全局 BINDINGS set，防并发重复绑定触发 pdfium-render 的 assert。
    static INIT_LOCK: Mutex<()> = Mutex::new(());
    let _guard = INIT_LOCK.lock().map_err(|e| {
        IngestError::Pdf(format!("PDFium 初始化锁 poisoned: {e}"))
    })?;
    // 持锁后重查：可能已被另一线程初始化。
    if PDFIUM.get().is_some() {
        return Ok(());
    }
    tracing::info!(?lib_path, "pdfium: binding to library");
    let bindings = Pdfium::bind_to_library(lib_path).map_err(|e| {
        IngestError::Pdf(format!(
            "加载 PDFium 动态库失败（{lib_path:?}）：{e}。请确认库文件存在且版本与 pdfium-render 的 pdfium_7881 feature 一致"
        ))
    })?;
    let pdfium = Pdfium::new(bindings);
    // 到这里已经持锁且唯一，直接 set（get_or_init 仅为兜底幂等）。
    let _ = PDFIUM.get_or_init(|| pdfium);
    tracing::info!("pdfium: initialized");
    Ok(())
}

/// 借用进程级 PDFium 单例（不持锁）。
///
/// **线程安全**：返回的 `&Pdfium` 虽是 `Sync`，但 PDFium C 库非线程安全。
/// 调用任何会触发 FFI 的操作（load/extract/render）前，必须先取 [`with_pdfium`]
/// 锁 guard 并在整个临界区持有。仅查询单例是否就绪等不触发 FFI 的场景可不持锁。
///
/// 未初始化时返回错误（调用方应确保 `src-tauri` 启动时已 `init_pdfium`）。
pub fn pdfium() -> IngestResult<&'static Pdfium> {
    PDFIUM.get().ok_or_else(|| {
        IngestError::Pdf(
            "PDFium 未初始化。请确保应用启动时已调用 ingest::init_pdfium 加载 PDFium 动态库".to_string(),
        )
    })
}
