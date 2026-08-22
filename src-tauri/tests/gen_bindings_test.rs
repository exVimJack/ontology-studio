//! 通过 `cargo test --test gen_bindings_test` 生成 tauri-specta TS 绑定。
//!
//! 为什么用集成测试而非 example：example 编译为独立 exe，链接 cdylib 时在当前
//! 环境遇 DLL 入口点缺失（STATUS_ENTRYPOINT_NOT_FOUND）；集成测试静态链接 rlib，
//! 在同进程内运行，无 DLL 问题。
//!
//! 用法（src-tauri 下）：`cargo-msvc.bat test --test gen_bindings_test -- --nocapture`

#[test]
fn gen_bindings() {
    let path = "../src/lib/ipc/bindings.ts";
    onto_studio_lib::gen_bindings(path).expect("failed to export bindings");
    eprintln!("[gen_bindings_test] wrote {path}");
}
