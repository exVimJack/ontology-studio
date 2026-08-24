// Hooks 层：useSkills.ts（决策 20）
// Skill 系统的 TanStack Query 封装。
//
// 两个查询粒度：
// - 全局列表（conversationId=null）：设置页用，显示全局禁用状态
// - 会话级列表（conversationId=xxx）：SkillPopover 用，显示会话内 enabled 开关
//
// 变更后统一 invalidate 两个 query key（全局 + 当前会话），保证状态一致。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc/commands";
import type { SkillDto, SkillSource } from "@/lib/domain";

/** 全局 skill 列表（设置页用）。conversationId=null 只返回全局状态。 */
const QK_SKILLS_GLOBAL = ["skills", "global"] as const;
/** 会话级 skill 列表（SkillPopover 用），含 conversation_enabled。 */
const qkSkillsConv = (cid: string) => ["skills", "conv", cid] as const;

/** 全局 skill 列表（设置页）。retry=1：扫描超时后尽快展示错误态供用户重试。 */
export function useSkillsGlobal() {
  return useQuery<SkillDto[]>({
    queryKey: QK_SKILLS_GLOBAL,
    queryFn: () => ipc.listSkills(null),
    retry: 1,
    retryDelay: 2000,
  });
}

/** 会话级 skill 列表（SkillPopover）。conversationId 为 null 时不查询。 */
export function useSkillsConversation(conversationId: string | null) {
  return useQuery<SkillDto[]>({
    queryKey: conversationId
      ? qkSkillsConv(conversationId)
      : ["skills", "conv", "none"],
    queryFn: () => ipc.listSkills(conversationId),
    enabled: !!conversationId,
    retry: 1,
    retryDelay: 2000,
  });
}

/** 导入本地 skill 目录。成功后刷新列表。 */
export function useImportSkillFromDir() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (srcPath: string) => ipc.importSkillFromDir(srcPath),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["skills"] });
    },
  });
}

/** 导入 zip skill。 */
export function useImportSkillFromZip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (zipPath: string) => ipc.importSkillFromZip(zipPath),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["skills"] });
    },
  });
}

/** 卸载导入的 skill。 */
export function useUninstallSkill() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (skillName: string) => ipc.uninstallSkill(skillName),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["skills"] });
    },
  });
}

/** 设置会话级 skill enabled（SkillPopover 开关）。 */
export function useSetSkillConversationEnabled(conversationId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: {
      skillName: string;
      source: SkillSource;
      enabled: boolean;
    }) =>
      ipc.setSkillConversationEnabled(
        conversationId,
        args.skillName,
        args.source,
        args.enabled,
      ),
    // 乐观更新：先改缓存，失败回滚
    onMutate: async ({ skillName, source, enabled }) => {
      const qk = qkSkillsConv(conversationId);
      await qc.cancelQueries({ queryKey: qk });
      const prev = qc.getQueryData<SkillDto[]>(qk);
      qc.setQueryData<SkillDto[]>(qk, (old) =>
        (old ?? []).map((s) =>
          s.name === skillName && s.source === source
            ? { ...s, conversation_enabled: enabled }
            : s,
        ),
      );
      return { prev };
    },
    onError: (_e, _vars, ctx) => {
      if (ctx?.prev) {
        qc.setQueryData(qkSkillsConv(conversationId), ctx.prev);
      }
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: ["skills"] });
    },
  });
}

/** 设置全局 skill 禁用（设置页开关）。 */
export function useSetSkillGloballyDisabled() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: { skillName: string; disabled: boolean }) =>
      ipc.setSkillGloballyDisabled(args.skillName, args.disabled),
    onMutate: async ({ skillName, disabled }) => {
      await qc.cancelQueries({ queryKey: QK_SKILLS_GLOBAL });
      const prev = qc.getQueryData<SkillDto[]>(QK_SKILLS_GLOBAL);
      qc.setQueryData<SkillDto[]>(QK_SKILLS_GLOBAL, (old) =>
        (old ?? []).map((s) =>
          s.name === skillName ? { ...s, globally_disabled: disabled } : s,
        ),
      );
      return { prev };
    },
    onError: (_e, _vars, ctx) => {
      if (ctx?.prev) qc.setQueryData(QK_SKILLS_GLOBAL, ctx.prev);
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: ["skills"] });
    },
  });
}
