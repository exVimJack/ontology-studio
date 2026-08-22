//! PDFium 动态库定位与初始化（决策 5）。
//!
//! 启动时（setup hook）调 `init(app_handle)`：
//! 1. 按当前编译 target 选对应平台库文件名
//! 2. 用 `PathResolver::resolve(BaseDirectory::Resource)` 定位打包的资源
//!    （dev 时指向 `src-tauri/resources/`，生产时指向安装包内资源目录）
//! 3. 调 `ingest::init_pdfium(path)` 注入进程级单例
//!
//! 失败处理：PDF 解析是核心能力，但库缺失不应阻断应用启动（用户可能暂不用 PDF）。
//! 故只记录错误，让 `ingest::PdfParser` 在实际解析时返回明确错误。

use ingest::init_pdfium;
use std::path::PathBuf;
use tauri::{Manager, Wry};
use tauri::path::BaseDirectory;

/// 当前编译目标对应的 PDFium 库文件相对路径（相对 resources 根）。
///
/// 与 `tauri.conf.json` 的 `bundle.resources` 配置一致。
fn platform_lib_rel() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    { "pdfium/win-x64/pdfium.dll" }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    { "pdfium/mac-arm64/libpdfium.dylib" }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    { "pdfium/mac-x64/libpdfium.dylib" }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    { "pdfium/linux-x64/libpdfium.so" }
}

/// 在 setup hook 中调用：定位并初始化 PDFium。
///
/// 返回库路径供日志/诊断；失败时记录 warn 但不 panic。
pub fn init(app: &tauri::AppHandle<Wry>) -> Option<PathBuf> {
    let rel = platform_lib_rel();

    // 优先：Tauri 资源解析（dev = src-tauri/resources/，生产 = 安装包资源目录）。
    // 注意：PathResolver::resolve 只做路径拼接，不校验文件是否存在；且 dev 下
    // BaseDirectory::Resource 指向 exe 所在目录（如 target/debug/）而非其下
    // resources/ 子目录，故 resolve("pdfium/...", Resource) 拼出的路径在 dev 下
    // 通常并不存在。必须显式 exists() 校验后才采用，否则会跳过兜底、把不存在
    // 的路径传给 LoadLibrary 而得到误导性的 LoadLibraryError。
    let path = app
        .path()
        .resolve(rel, BaseDirectory::Resource)
        .ok()
        .filter(|p| p.exists())
        // 兜底 1：dev 下 Tauri 会把 bundle.resources 拷到 <exe-dir>/resources/。
        // Resource 解析不带 resources/ 段，这里补上。
        .or_else(|| {
            app.path()
                .resource_dir()
                .ok()
                .map(|d| d.join("resources").join(rel))
                .filter(|p| p.exists())
        })
        // 兜底 2：源码树（CARGO_MANIFEST_DIR = src-tauri/，resources/ 在其下）。
        // 直接 cargo run 而非 tauri dev 时 resource_dir 可能不含拷贝资源，此路径兜底。
        .or_else(|| {
            let manifest = env!("CARGO_MANIFEST_DIR");
            let p = std::path::Path::new(manifest).join("resources").join(rel);
            if p.exists() { Some(p) } else { None }
        });

    let path = match path {
        Some(p) => p,
        None => {
            tracing::error!(
                rel,
                "pdfium: 库文件未找到。请运行 scripts/fetch-pdfium.sh 下载，或检查 tauri.conf.json resources 配置"
            );
            return None;
        }
    };

    tracing::info!(path = %path.display(), "pdfium: initializing");
    match init_pdfium(&path) {
        Ok(()) => {
            tracing::info!("pdfium: initialized");
            Some(path)
        }
        Err(e) => {
            tracing::error!(error = %e, path = %path.display(), "pdfium: init failed");
            None
        }
    }
}
