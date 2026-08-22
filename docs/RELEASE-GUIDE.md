# ontology-studio 独立仓库发布指南

> 把 `apps/onto-studio` 剥离成独立仓库 `ontology-studio`，GitHub Actions 编译 Windows/macOS 包，产物回传 GitCode Release 供国内下载。

## 架构概览

```
GitHub (exVimJack/ontology-studio)     GitCode (JasonSmiths/ontology-studio)
┌──────────────────────────────┐     ┌──────────────────────────────┐
│  代码主仓 + CI 编译           │ ──Pull 镜像──▶ │  代码镜像 + Release 下载页      │
│  windows-latest → exe         │     │  (国内用户在这里下载)         │
│  macos-14      → dmg (arm64)  │     │                              │
│  macos-15-intel→ dmg (x64)    │     │                              │
└──────────────────────────────┘     └──────────────────────────────┘
              │                                    ▲
              │  build 完调 GitCode API            │
              └──── 上传 exe/dmg 到 GitCode Release ┘
```

**核心原则**（来自 AGENTS.md 红线）：GitCode 只托管代码镜像 + Release 下载页；编译全靠 GitHub 公有 Runner（GitCode CI 不提供跨平台公有 runner）。

---

## 一、已完成（我已改好，你无需再动）

| 改动 | 文件 | 说明 |
| --- | --- | --- |
| 仓库名元数据改名 | `Cargo.toml`、`src-tauri/Cargo.toml`、`package.json` | `onto-studio` → `ontology-studio`（仅元数据） |
| 重写发布 workflow | `.github/workflows/release.yml` | 修正方案1所有坑 + 保留 PDFium fetch + 三平台矩阵 |
| Git 剥离脚本 | `scripts/init-standalone-repo.sh` | `git init` + 双 remote + 首次提交 |

### 🚫 没改的东西（故意保留，避免破坏运行时）

- **`identifier: com.onto-studio.app`**（tauri.conf.json）—— 改了会让已安装用户数据目录丢失
- **`~/.onto-studio/` 运行时路径**（代码几十处）—— 改了用户 db 和 skill 全丢
- **`onto_studio_lib` Rust 库名** —— 代码符号，改了要动 `[lib]` 引用
- **文档里的 `onto-studio` 文字**（ARCHITECTURE.md / PROGRESS.md / docs/）—— 独立工程，不阻塞打包，后续可单独批量替换

### 📝 release.yml 相比方案1原版的关键修正

| 问题 | 原方案1 | 修正后 |
| --- | --- | --- |
| secrets 块位置 | 嵌在 `with:` 后（YAML 解析错） | 与 `with:` 同级（正确） |
| bash globstar | `**/*.{exe,...}` 跨目录匹配不到 | `find ... -name '*.exe'` 递归（根治） |
| 第三方 action 依赖 | `nvdacn/sync_to_gitcode` | 纯 curl（零外部依赖） |
| tauri-action | `tagName: ""`（空字符串可能告警） | 不传 tagName（纯构建，官方推荐） |
| macOS Intel runner | `macos-13`（**2025-12 已下线**） | `macos-15-intel`（GitHub 推荐，可用到 2027-08） |
| 失败可观测性 | 第三方 action 黑盒 | curl 输出完整响应体 |
| artifact 缺失保护 | 无 | `if-no-files-found: error` |

---

## 二、需要你手动操作的步骤（按顺序）

### 步骤 1：在 GitHub 建空仓库

1. 打开 <https://github.com/new>
2. Owner: `exVimJack`，Repository name: `ontology-studio`
3. **Public**（公开仓 Actions 无限免费分钟）
4. **不要勾选** "Add a README" / ".gitignore" / "license" —— 空仓才能首次推送
5. Create repository

### 步骤 2：在 GitCode 建空仓库

1. 打开 <https://gitcode.com/projects/new>
2. 仓库路径: `JasonSmiths/ontology-studio`
3. **开源**
4. **不要初始化** README / .gitignore / license
5. 创建

### 步骤 3：生成 GitCode 个人访问令牌

1. <https://gitcode.com/profile/personal-access-tokens> （或 设置 → 个人令牌 Classic）
2. 创建令牌，**权限勾 `repo`**（完整仓库权限）
3. 复制 token（**只显示一次**，保存好）

### 步骤 4：在 GitHub 配置 Secret

1. 进入 <https://github.com/exVimJack/ontology-studio/settings/secrets/actions>
2. New repository secret：
   - Name: `GITCODE_TOKEN`
   - Value: 粘贴上一步的 GitCode token
3. （可选）配置 `GH_PROXY` 变量加速 PDFium 下载：
   - 进入 Settings → Secrets and variables → **Variables**（不是 Secrets）
   - New variable: Name=`GH_PROXY`，Value=`https://ghfast.top/`（或你常用的 GitHub 镜像）

### 步骤 5：运行剥离脚本（在本地执行）

```bash
cd /Users/thinkpiggy/codes/union_agent/apps/onto-studio
bash scripts/init-standalone-repo.sh
```

脚本会：

- 在 `apps/onto-studio/` 下 `git init` 新仓（独立历史，不带 union_agent 的提交历史）
- 配置 `github` + `gitcode` 双 remote
- 首次 commit
- 推送到两边

> ⚠️ 脚本推送前会问你确认。如果两边仓库非空会冲突，务必保证步骤 1、2 建的是**空仓**。

### 步骤 6：配置 GitCode Pull 镜像（自动同步后续代码）

进入 GitCode 项目 → 项目设置 → 仓库镜像 → 添加 **Pull 镜像**：

- 镜像操作：**Pull 拉取**
- 远程仓库地址：`https://github.com/exVimJack/ontology-studio.git`
- 开启自动同步

配置后，GitHub 有新提交/打 tag，GitCode 自动拉取。

### 步骤 7：验证流水线（打测试 tag）

```bash
cd /Users/thinkpiggy/codes/union_agent/apps/onto-studio
git tag v0.0.1-test
git push github v0.0.1-test --tags
```

然后：

1. 看 <https://github.com/exVimJack/ontology-studio/actions> —— 三个 build job 应该都跑起来
2. 等 build job 全绿（约 15-25 分钟，Rust 首次编译慢）
3. 看 sync_gitcode_release job 是否成功
4. 打开 <https://gitcode.com/JasonSmiths/ontology-studio/releases> —— 应看到 `v0.0.1-test` Release，含 exe × 1 + dmg × 2

如果失败，看 Actions 日志里对应 step 的 `cat /tmp/*.json` 输出（workflow 设计为失败时打印完整 GitCode API 响应体）。

### 步骤 8：正式发版

测试通过后：

```bash
git tag v0.1.0
git push github v0.1.0 --tags
```

---

## 三、常见问题

### Q: 为什么 identifier 还叫 `com.onto-studio.app` 不改成 `com.ontology-studio.app`？

macOS/Windows 用 identifier 定位用户数据目录：

- macOS: `~/Library/Application Support/com.onto-studio.app/`
- Windows: `%APPDATA%\com.onto-studio.app\`

**改 identifier = 已安装用户升级后数据全丢**（新旧 identifier 互不认）。identifier 应长期稳定，不随仓库名变。这不是疏忽，是保护用户数据的故意决策。

### Q: 为什么 `~/.onto-studio/` 路径不改？

同上，这是用户磁盘上的实际目录，存着 db 和 skill。改了用户升级后找不到自己的数据。运行时路径 = 契约，不随仓库改名。

### Q: macOS 包没签名，用户下载会报"无法验证开发者"吗？

会。这是方案1的已知限制（坑清单 #2）。用户需右键 → 打开，或在系统设置允许。如有 Apple 开发者账号（\$99/年）想签名，需另配 `APPLE_CERTIFICATE` / `APPLE_ID` secrets 并修改 workflow 的 tauri-action 配置——当前未做。

### Q: Linux 包呢？

没做。onto-studio 是桌面知识工作台，Linux 桌面用户极少。现有矩阵只 windows + macOS arm64 + macOS x64。如需 Linux，在 `release.yml` 的 matrix 加 `ubuntu-22.04` 即可。

### Q: GitCode Pull 镜像会同步 Release 吗？

**不会**。Pull 镜像只同步代码（分支+tag），**Release 是独立实体，不走镜像**。所以 Release 靠 workflow 里的 sync_gitcode_release job 通过 API 直接创建。这是设计如此，不是 bug。

### Q: 首次推送太慢？

Rust workspace + src-tauri 编译产物大，但 `.gitignore` 已排除 `target/` 和二进制 PDFium。首次推送应该只有源码（约几十 MB）。如果卡住检查是否误带了 `target-d/` 或大文件：

```bash
du -sh --exclude=target --exclude=node_modules --exclude=dist .
git ls-files | xargs du -ch | sort -h | tail -20  # 看最大的 tracked 文件
```

---

## 四、回滚

如果剥离失败想回到 union_agent 子目录状态：

```bash
cd /Users/thinkpiggy/codes/union_agent/apps/onto-studio
rm -rf .git                          # 删掉独立仓的 .git
# 元数据改动（Cargo.toml/package.json）已在 union_agent 父仓库历史里，git checkout 即可恢复
cd /Users/thinkpiggy/codes/union_agent
git checkout -- apps/onto-studio/    # 恢复父仓库视角
```

GitHub/GitCode 的空仓可手动删除。
