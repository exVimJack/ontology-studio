//! PDFium 资源定位 + init 端到端验证（决策 5）。
//!
//! 验证 dev 场景下 src-tauri 的 pdfium 资源解析兜底逻辑：
//! BaseDirectory::Resource 在 dev 指向 target/debug（不含资源），
//! 兜底用 CARGO_MANIFEST_DIR/resources/ 定位。此测试模拟该兜底路径。

#[test]
fn pdfium_resource_resolves_and_inits() {
    // 模拟 pdfium::platform_lib_rel() + 兜底逻辑
    let manifest = env!("CARGO_MANIFEST_DIR");
    let rel = "pdfium/win-x64/pdfium.dll";
    let path = std::path::Path::new(manifest).join("resources").join(rel);

    if !path.exists() {
        eprintln!(
            "[skip] {} 不存在（非 Windows x64 或未运行 fetch-pdfium），跳过",
            path.display()
        );
        return;
    }

    // 调真实 init_pdfium，验证能加载库
    ingest::init_pdfium(&path).expect("init_pdfium 应成功");

    // 验证单例已就位（第二次调应幂等返回 Ok）
    ingest::init_pdfium(&path).expect("init_pdfium 幂等调用应成功");
    eprintln!("✓ PDFium 资源定位 + init 验证通过：{}", path.display());
}
