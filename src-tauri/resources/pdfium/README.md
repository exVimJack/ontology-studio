# PDFium 预编译动态库（决策 5）

本目录存放随 Tauri 资源打包的 PDFium 动态库，供 `crates/ingest` 的 pdfium-render FFI 运行时加载。

## 版本锁定（关键）

PDFium 动态库版本必须与 `crates/ingest/Cargo.toml` 中 `pdfium-render` 的 feature **严格一致**（当前 `pdfium_7881` ↔ bblanchon `chromium/7881`）。不匹配会导致 `bind_to_library` 时 missing-symbol 崩溃。

升级 pdfium-render 时，同步：
1. 改 `crates/ingest/Cargo.toml` 的 feature（如 `pdfium_7961`）
2. 改 `scripts/fetch-pdfium.sh` / `.bat` 的 `VERSION`
3. 重跑下载脚本

## 目录结构

```
src-tauri/resources/pdfium/
├── win-x64/pdfium.dll
├── mac-arm64/libpdfium.dylib
├── mac-x64/libpdfium.dylib
└── linux-x64/libpdfium.so
```

> **动态库一律不入库**（各约 7MB，二进制产物）。所有平台库均由 CI 或开发者本地运行 `scripts/fetch-pdfium.sh` / `.bat` 拉取，已由 `.gitignore` 忽略。

## 首次开发前必读

clone 仓库后，PDFium 动态库**不在仓库中**，需手动下载一次（CI 会自动执行此步）：

- Windows：`scripts\fetch-pdfium.bat`
- macOS / Linux：`./scripts/fetch-pdfium.sh`

**不下载也能启动应用**：`src-tauri/src/pdfium.rs` 的 setup hook 在库缺失时只记录 warn，不阻断启动；仅在调用 PDF 解析功能时才会返回明确错误。

```bash
# 当前平台
./scripts/fetch-pdfium.sh

# 全平台（CI 用）
./scripts/fetch-pdfium.sh all

# 镜像加速（github 直连慢时）
GH_PROXY=https://ghfast.top ./scripts/fetch-pdfium.sh
```

Windows：`scripts\fetch-pdfium.bat`

## 许可证

- pdfium-render crate：MIT OR Apache-2.0
- PDFium 库本身：BSD-3-Clause（Google）
- bblanchon/pdfium-binaries 打包脚本：MIT

均符合项目原则 3（宽松许可，非 GPL）。

## 运行时加载

`src-tauri` 启动时（setup hook）通过 `tauri::path::PathResolver::resolve("pdfium/<platform-lib>", BaseDirectory::Resource)` 定位库，调 `ingest::init_pdfium(path)`。dev 模式资源在源码 `src-tauri/resources/` 下，Tauri 自动解析。
