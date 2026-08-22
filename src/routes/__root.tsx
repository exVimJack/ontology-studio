// 路由根布局（ARCHITECTURE.md 决策 F4 文件式路由）
// 原 App.tsx 逻辑下沉到此：TitleBar + AppShell(三栏,中间 Outlet) + CommandPalette
// + FileDropZone + Settings 覆盖层 + 全局快捷键 + 主题 effect。
//
// 路由参数为会话切换的唯一真相源（无 Zustand 同步）：
//   - currentConversationId 由 useCurrentConversationId() hook 从路由 params 派生
//   - 选会话/新建/删除统一走 navigate()
//   - 切会话清理 ingest-store 残留任务的副作用，由 useCurrentConversationId 的 effect 触发

import { useEffect } from "react";
import {
  createRootRouteWithContext,
  useNavigate,
} from "@tanstack/react-router";
import { TitleBar } from "@/components/shell/TitleBar";
import { AppShell } from "@/components/shell/AppShell";
import { CommandPalette } from "@/components/shell/CommandPalette";
import { SettingsView } from "@/components/settings/SettingsView";
import { FederationView } from "@/components/federation/FederationView";
import { SkillView } from "@/components/skill/SkillView";
import { OntologyView } from "@/components/ontology/OntologyView";
import { FileDropZone } from "@/components/library/FileDropZone";
import { useUiStore, applyTheme } from "@/stores/ui-store";
import { useCreateConversation } from "@/hooks/useConversations";
import { useCurrentConversationId } from "@/hooks/useCurrentConversationId";
import { useIngestStore } from "@/stores/ingest-store";

export const Route = createRootRouteWithContext()({
  component: RootComponent,
});

function RootComponent() {
  const setPaletteOpen = useUiStore((s) => s.setCommandPaletteOpen);
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen);
  const createConv = useCreateConversation();
  const navigate = useNavigate();
  const theme = useUiStore((s) => s.theme);
  const settingsOpen = useUiStore((s) => s.settingsOpen);
  const federationOpen = useUiStore((s) => s.federationOpen);
  const skillsOpen = useUiStore((s) => s.skillsOpen);
  const ontologyOpen = useUiStore((s) => s.ontologyOpen);

  // 切会话清理上一会话残留的已结束摄入任务（原 setCurrentConversation 副作用，
  // 现由路由派生的 id 变化触发）。
  // 切会话不再自动清摄入任务（原 clearFinished 会连失败记录一起清掉，
  // 导致用户切走再回来“文件去哪了”）。失败记录按会话隔离由 IngestStatusBoard 的
  // conversationId 过滤实现，清理交看板“清除”按钮（clearResolved）主动触发。
  const conversationId = useCurrentConversationId();
  useEffect(() => {
    // 仅清 Done（看板本就不显示的瞬态态），保留 Error/Cancelled 供回看。
    useIngestStore.getState().clearFinished();
  }, [conversationId]);

  // 主题：初始应用 + 监听变化
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // 监听系统主题变化（仅 system 模式）
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => {
      if (useUiStore.getState().theme === "system") applyTheme("system");
    };
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  // 全局快捷键
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(!useUiStore.getState().commandPaletteOpen);
      } else if (mod && e.key.toLowerCase() === "n" && !e.shiftKey) {
        // ⌘N 新建会话（输入框聚焦时不抢）
        const tag = (e.target as HTMLElement)?.tagName;
        if (tag !== "INPUT" && tag !== "TEXTAREA") {
          e.preventDefault();
          createConv.mutate({});
        }
      } else if (mod && e.key === ",") {
        // ⌘, 打开设置
        e.preventDefault();
        setSettingsOpen(true);
      } else if (mod && e.key.toLowerCase() === "b") {
        // ⌘B 文件库（§16 快捷键）
        e.preventDefault();
        navigate({ to: "/library" });
      } else if (mod && e.shiftKey && e.key.toLowerCase() === "f") {
        // ⌘Shift+F 联邦查询
        e.preventDefault();
        useUiStore.getState().setFederationOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setPaletteOpen, createConv, navigate]);

  return (
    <div className="flex h-full flex-col">
      <TitleBar />
      <div className="min-h-0 flex-1">
        <AppShell />
      </div>
      <CommandPalette />
      <FileDropZone />
      {settingsOpen && <SettingsView onClose={() => setSettingsOpen(false)} />}
      {federationOpen && <FederationView />}
      {skillsOpen && <SkillView />}
      {ontologyOpen && <OntologyView />}
    </div>
  );
}
