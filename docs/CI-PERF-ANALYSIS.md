# Windows CI 提速分析（基于 cargo --timings 实测数据）

> 基线：Windows 15m06s，macOS 5m52s。本地 macOS `cargo build --release --timings` 总耗时 297s。

## 核心发现：15 分钟耗在哪

### 1. PDFium 不是瓶颈（你的最大怀疑已排除）

PDFium 是**预编译动态库**（`bblanchon/pdfium-binaries` 项目的 `.dll`/`.dylib`），随 Tauri `resources` 打包，运行时由 `pdfium-render` FFI 加载。**编译成本 = 0**，不存在"每次重新 C/C++ build PDFium"的问题。

```
src-tauri/resources/pdfium/win-x64/pdfium.dll     # 预编译，直接打包
src-tauri/resources/pdfium/mac-arm64/libpdfium.dylib
crates/ingest: pdfium-render = "0.9.3"              # 纯 FFI 绑定 crate，不编译 C++
```

### 2. 真正的瓶颈：C/C++ 编译的 `-sys` crate（本地 Top 耗时）

| 排名 | build-script | 本地耗时 | 引入路径 | Windows 预估 |
| --- | --- | --- | --- | --- |
| 1 | **aws-lc-sys** | **104s** | reqwest→rustls→aws-lc-rs | **可能 200-300s**（MSVC+Defender） |
| 2 | zstd-sys | 46s | datafusion→parquet→zstd | ~100s |
| 3 | libsqlite3-sys | 29s | rusqlite (bundled C 源码) | ~60s |
| 4 | liblzma-sys | 14s | 压缩依赖 | ~30s |
| 5 | ring | 7s | rustls-webpki（与 aws-lc 并存！） | ~15s |

**aws-lc-sys 占本地编译的 1/3（104s/297s），是单一最大瓶颈。** 在 Windows 上 C 编译更慢，占比可能更高。

### 3. 两个 crypto 后端重复编译

- `aws-lc-rs`：reqwest/rig 的 `rustls` feature 默认拉入（无 provider 选择，rustls 默认 aws-lc-rs）
- `ring`：sqlx 的 `tls-rustls-ring` 显式选择 + `rustls-webpki` 依赖

**rustls 只需要一个 crypto provider，同时编译两个纯属浪费。**

### 4. profile.release 用了 `lto = "thin"`

```toml
[profile.release]
codegen-units = 16
lto = "thin"          # ← 增加 link time
opt-level = 3
panic = "abort"
strip = true
```

thin LTO 比无 LTO 慢，比 fat LTO 快。

### 5. 普通 Rust crate 编译时间都很短

Top 25 里没有超过 5s 的普通 Rust crate——datafusion/arrow 全家桶虽然依赖多，但单个 crate 编译并不慢。**大头不在 Rust 侧，在 C 编译侧。**

---

## 优化清单（按 预计收益 / 风险 / 是否值得试 排序）

### 第一批：确定值得试（高收益低风险）

#### ✅ 实验 A：去 aws-lc-rs，统一用 ring 后端（预计省 100-200s）

**预计收益**：Windows 省 2-4 分钟（aws-lc-sys 是最大单一 C 编译）

**做法**：reqwest 的 `rustls` feature 改为 `rustls-tls-webpki-roots`（强制 ring 后端）

```toml
# crates/agent-core/Cargo.toml
reqwest = { version = "0.13", default-features = false, features = [
    "rustls-tls-webpki-roots",  # 原 "rustls"（无 provider，默认拉 aws-lc-rs）
    "json",
    "stream",
] }
```

**风险**：低。ring 和 aws-lc-rs 都是 rustls 的 crypto provider，功能等价（TLS 加密）。ring 更轻量、更广泛使用。AGENTS.md 原则 1（禁 OpenSSL）不受影响——都是 rustls 栈。

**验证**：改后跑 `cargo tree -i aws-lc-rs` 应无输出，`cargo tree -i ring` 仍存在。

#### ✅ 实验 B：测 `lto = false`（你点名要测的项）

**预计收益**：Windows link 阶段省 1-2 分钟（thin LTO 增加 link time）

**做法**：

```toml
[profile.release]
lto = false  # 原 "thin"
```

**风险**：低-中。关 LTO 会轻微降低运行时性能（内联优化变少），但 Tauri 桌面应用非性能敏感，1-3% 下降可接受。可先测 `false`，再测 `thin` 对比。

#### ✅ 实验 C：测 `opt-level = 2`

**预计收益**：1-2 分钟。opt-level 3 比 2 编译更慢，收益主要在运行时数值计算密集场景。

**做法**：

```toml
[profile.release]
opt-level = 2  # 原 3
```

**风险**：低。桌面应用 opt-level 2 已足够。

### 第二批：值得测但需先做实验 A/B/C 建立新基线

#### 🔬 实验 D：`-j` 线程数 A/B（你特别建议的）

**预计收益**：不确定，需实测。Windows 4 CPU，4 个编译进程 + C 编译 + Defender 可能 IO 争抢。

**做法**：在 workflow 加 `CARGO_BUILD_JOBS` env var 测 `-j 2/3/4`。

**风险**：零（只影响 CI 编译，不影响产物）。

#### 🔬 实验 E：build-override 降级

**预计收益**：30-60s。build.rs / proc-macro 不需要 release 级优化。

**做法**：

```toml
[profile.release.build-override]
opt-level = 0
codegen-units = 256
debug = false
```

**风险**：零。build script 只在编译期运行，性能无关。

#### 🔬 实验 F：datafusion 的 zstd/lzma 是否可关

**预计收益**：60-90s（zstd-sys 46s + liblzma-sys 14s）。但 datafusion 三期才用，现在是否真需要？

**做法**：检查 federation crate 是否在 release 构建里被全量编译。若是可选，加 feature gate。

**风险**：中。需确认 federation 是否被 agent-core 强依赖。

### 第三批：已验证不适用（勿重复尝试）

- ❌ rust-lld：PDFium 原生库上链接卡死
- ❌ Dev Drive：CARGO_TARGET_DIR 重定向致 swatinem cache 失配，全量重编
- ❌ sccache：不缓存 cdylib + 不缓存 incremental，与 Tauri 三重冲突
- ❌ CARGO_TARGET_DIR 重定向：破坏 cache

### 不在本清单（因为已排除）

- PDFium 独立 artifact（它本就预编译，无需此操作）
- windows-2022 vs 2025（收益不确定，先做确定性高的实验 A/B/C）
- Self-hosted / Larger runner（成本高，最后才考虑）

---

## 建议执行顺序

1. **先做实验 A**（去 aws-lc-rs）→ 测一次 CI，看是否省 2-4 分钟
2. **再做实验 B**（lto=false）+ **实验 C**（opt-level=2）一起测 → 看是否再省 2-3 分钟
3. 新基线确立后，做实验 D（-j A/B）微调
4. 实验 E/F 按需

**预期目标**：Windows 15m → 10-12m（实验 A+B+C 后），再经实验 D 微调可能进 10m 内。
