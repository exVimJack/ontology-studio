#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────
# ontology-studio 独立仓库初始化脚本
#
# 作用：把 apps/onto-studio（union_agent 的子目录）剥离成独立 Git 仓库，
#       推送到 GitHub (exVimJack/ontology-studio) + GitCode (JasonSmiths/ontology-studio)。
#
# 前置条件（必须先完成，见 RELEASE-GUIDE.md）：
#   1. GitHub 已建空仓 https://github.com/exVimJack/ontology-studio （不要初始化 README）
#   2. GitCode 已建空仓 https://gitcode.com/JasonSmiths/ontology-studio （不要初始化 README）
#   3. GitHub Secret GITCODE_TOKEN 已配置（repo 权限的 GitCode 个人访问令牌）
#
# 本脚本幂等：重复运行不会破坏已有 .git
# ─────────────────────────────────────────────────────────────
set -euo pipefail

PROJECT_DIR="/Users/thinkpiggy/codes/union_agent/apps/onto-studio"
GITHUB_REMOTE="git@github.com:exVimJack/ontology-studio.git"
GITCODE_REMOTE="git@gitcode.com:JasonSmiths/ontology-studio.git"

cd "$PROJECT_DIR"

echo "▸ 当前目录: $(pwd)"
echo "▸ 项目大小（不含 target/node_modules）："
du -sh --exclude=target --exclude=node_modules --exclude=dist . 2>/dev/null || du -sh .

# ── 1. 初始化独立 .git（如果不存在）──
if [ -d ".git" ]; then
  echo "▸ .git 已存在，跳过 init（幂等）"
else
  echo "▸ git init 新仓库（独立历史，不带 union_agent 提交历史）"
  git init -b main
fi

# ── 2. 配置双 remote ──
echo "▸ 配置 remote："
# GitHub
if git remote get-url github >/dev/null 2>&1; then
  echo "  github remote 已存在，更新为 $GITHUB_REMOTE"
  git remote set-url github "$GITHUB_REMOTE"
else
  git remote add github "$GITHUB_REMOTE"
  echo "  ✓ 已添加 github → $GITHUB_REMOTE"
fi
# GitCode
if git remote get-url gitcode >/dev/null 2>&1; then
  echo "  gitcode remote 已存在，更新为 $GITCODE_REMOTE"
  git remote set-url gitcode "$GITCODE_REMOTE"
else
  git remote add gitcode "$GITCODE_REMOTE"
  echo "  ✓ 已添加 gitcode → $GITCODE_REMOTE"
fi

git remote -v

# ── 3. 首次提交（仅当无 commit 时）──
if git rev-parse HEAD >/dev/null 2>&1; then
  echo "▸ 已有 commit，跳过首次提交（幂等）"
else
  echo "▸ git add 全部文件（.gitignore 已排除 target/node_modules/pdfium 二进制）"
  git add -A
  echo "▸ 暂存文件数: $(git diff --cached --numstat | wc -l | tr -d ' ')"
  git commit -m "chore: initialize ontology-studio (独立仓库，从 union_agent/apps/onto-studio 剥离)

原属 union_agent monorepo 子目录，现剥离为独立仓库。
- 仓库: github.com/exVimJack/ontology-studio + gitcode.com/JasonSmiths/ontology-studio
- 元数据改名 onto-studio → ontology-studio（Cargo.toml / package.json）
- 运行时路径 ~/.onto-studio/ 和 bundle identifier com.onto-studio.app 保持不变（保护已安装用户数据）
- CI: GitHub Actions 三平台编译 + 产物回传 GitCode Release（见 .github/workflows/release.yml）"
fi

# ── 4. 推送双 remote ──
echo ""
echo "▸ 即将推送到："
echo "    github : $GITHUB_REMOTE"
echo "    gitcode: $GITCODE_REMOTE"
echo ""
read -r -p "确认推送？两边都必须是已建好的空仓（否则会冲突）。继续？[y/N] " ans
case "$ans" in
y | Y | yes) ;;
*)
  echo "已取消"
  exit 0
  ;;
esac

echo "▸ 推送 main 分支到 github..."
# GitHub 仓为空，首次 push 直接成功
git push -u github main

echo "▸ 推送 main 分支到 gitcode..."
# GitCode 仓建仓时勾选了"初始化 README"，远端有初始 commit (d2f20b7) 本地没有。
# 这是空仓基线建立场景，用户已授权 --force 覆盖（会覆盖远端那个自动生成的 README commit）。
# 注意：force 覆盖仅限首次建仓基线，后续日常推送绝不使用 force。
git push -u gitcode main --force

echo ""
echo "🎉 推送完成！"
echo ""
echo "下一步："
echo "  1. 在 GitCode 项目设置 → 仓库镜像 → 添加 Pull 镜像，源填 GitHub 地址（自动同步后续代码+tag）"
echo "  2. 打测试 tag 验证流水线："
echo "     git tag v0.0.1-test"
echo "     git push github v0.0.1-test --tags"
echo "  3. 看 GitHub Actions 日志确认 exe/dmg 产出 + GitCode Release 出现"
