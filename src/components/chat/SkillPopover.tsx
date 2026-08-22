// UI 层：SkillPopover.tsx（决策 20）
// Composer 工具行内的会话级 Skill 开关 popover。
//
// 由原 Inspector 第三栏的 SkillTogglePanel 演化而来（方案 C：砍掉第三栏，
// skill 开关收进 Composer popover）。
//
// 与技能主视图 SkillView 的区别：
// - SkillView 管「全局禁用」（层次 2）+ 导入/卸载（独立覆盖层）
// - 这里管「会话级 enabled」（层次 3）：精细控制当前会话内哪些 skill 生效
//
// 三层 disable 语义在 UI 的体现：
// - 全局禁用 → 灰显 + 禁用开关 + 提示「去技能页解除全局禁用」
// - disable_model_invocation → 显示盾牌图标，开关默认 off，开启后=@ 激活路径
// - 会话级 enabled → 开关状态（None=按 source 默认，true/false=显式覆盖）
//
// 形态：工具行按钮（显示激活数）+ 点开下拉面板（按来源分组 + 开关）。

import { useMemo, useState } from "react"
import { Zap, Shield, Settings2 } from "lucide-react"
import { useSkillsConversation, useSetSkillConversationEnabled } from "@/hooks/useSkills"
import { SKILL_SOURCE_META, isSkillActive, canToggleInConversation } from "@/lib/domain"
import type { SkillDto, SkillSource } from "@/lib/domain"
import { useUiStore } from "@/stores/ui-store"

/** 来源分组顺序。 */
const SOURCE_ORDER: SkillSource[] = ["builtin", "imported", "external-read-only", "project"]

export function SkillPopover({ conversationId }: { conversationId: string }) {
  const [open, setOpen] = useState(false)
  const { data: skills = [] } = useSkillsConversation(conversationId)
  const setEnabled = useSetSkillConversationEnabled(conversationId)
  const setSkillsOpen = useUiStore((s) => s.setSkillsOpen)

  const activeCount = useMemo(() => skills.filter(isSkillActive).length, [skills])

  // 按来源分组
  const grouped = useMemo(() => {
    const m = new Map<SkillSource, SkillDto[]>()
    for (const s of skills) {
      const arr = m.get(s.source) ?? []
      arr.push(s)
      m.set(s.source, arr)
    }
    return m
  }, [skills])

  const onToggle = (s: SkillDto) => {
    if (!canToggleInConversation(s)) return
    const next = !isSkillActive(s)
    setEnabled.mutate({ skillName: s.name, source: s.source, enabled: next })
  }

  return (
    <div className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        title="会话技能"
        aria-pressed={open}
        className={`flex shrink-0 items-center gap-1 rounded-md px-2 py-1.5 text-xs transition-colors ${
          activeCount > 0
            ? "bg-accent/15 text-accent"
            : "text-fg-subtle hover:bg-bg-hover hover:text-fg"
        }`}
      >
        <Zap size={14} />
        <span>技能</span>
        {activeCount > 0 && (
          <span className="rounded-full bg-accent/20 px-1 text-[10px] font-medium leading-none">
            {activeCount}
          </span>
        )}
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute bottom-full left-0 z-50 mb-1 w-72 rounded-lg border border-border bg-bg-elevated shadow-xl">
            <div className="flex items-center gap-1.5 border-b border-border px-3 py-2 text-xs font-medium text-fg">
              <Zap size={12} /> 会话技能
              <span className="ml-auto text-[10px] font-normal text-fg-subtle">
                {activeCount} 激活 / {skills.length}
              </span>
            </div>

            <div className="max-h-80 overflow-auto p-2">
              {skills.length === 0 ? (
                <div className="flex flex-col items-center justify-center gap-1.5 px-3 py-6 text-center">
                  <Zap size={20} className="text-fg-subtle" />
                  <p className="text-xs text-fg-subtle">未发现 skill</p>
                  <button
                    onClick={() => {
                      setOpen(false)
                      setSkillsOpen(true)
                    }}
                    className="mt-1 flex items-center gap-1 text-[11px] text-accent hover:underline"
                  >
                    <Settings2 size={10} /> 前往技能页
                  </button>
                </div>
              ) : (
                <div className="space-y-3">
                  {SOURCE_ORDER.map((src) => {
                    const list = grouped.get(src)
                    if (!list || list.length === 0) return null
                    const meta = SKILL_SOURCE_META[src]
                    return (
                      <div key={src}>
                        <div className="mb-1 px-0.5 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                          {meta.label}（{list.length}）
                        </div>
                        <ul className="space-y-1">
                          {list.map((s) => {
                            const active = isSkillActive(s)
                            const canToggle = canToggleInConversation(s)
                            const meta = SKILL_SOURCE_META[s.source]
                            return (
                              <li
                                key={`${s.source}:${s.name}`}
                                className={`rounded-md border border-border px-2 py-1.5 ${
                                  s.globally_disabled ? "opacity-50" : ""
                                }`}
                              >
                                <div className="flex items-center gap-1.5">
                                  <button
                                    role="switch"
                                    aria-checked={active}
                                    aria-label={`${active ? "关闭" : "开启"} ${s.name}`}
                                    disabled={!canToggle}
                                    onClick={() => onToggle(s)}
                                    className={`relative h-4 w-7 shrink-0 rounded-full transition-colors disabled:cursor-not-allowed ${
                                      active ? "bg-accent" : "bg-border"
                                    }`}
                                  >
                                    <span
                                      className={`absolute top-0.5 h-3 w-3 rounded-full bg-white transition-transform ${
                                        active ? "left-3.5" : "left-0.5"
                                      }`}
                                    />
                                  </button>
                                  <div className="min-w-0 flex-1">
                                    <div className="flex items-center gap-1">
                                      <span className="truncate text-[11px] font-medium text-fg">{s.name}</span>
                                      {s.disable_model_invocation && (
                                        <Shield
                                          size={9}
                                          className="shrink-0 text-amber-500"
                                          aria-label="需 @skill-name 手动激活"
                                        />
                                      )}
                                    </div>
                                    <p className="truncate text-[10px] text-fg-subtle" title={s.description}>
                                      {s.description}
                                    </p>
                                  </div>
                                  <span
                                    className={`shrink-0 rounded border px-1 py-0 text-[9px] ${meta.badgeCls}`}
                                  >
                                    {meta.label}
                                  </span>
                                </div>
                                {s.globally_disabled && (
                                  <p className="mt-1 text-[9px] text-rose-500">
                                    已全局禁用（技能页可解除）
                                  </p>
                                )}
                              </li>
                            )
                          })}
                        </ul>
                      </div>
                    )
                  })}
                </div>
              )}
            </div>

            <div className="flex items-center justify-between border-t border-border px-3 py-2">
              <button
                onClick={() => {
                  setOpen(false)
                  setSkillsOpen(true)
                }}
                className="flex items-center gap-1 text-[10px] text-fg-subtle hover:text-accent"
              >
                <Settings2 size={10} /> 管理技能
              </button>
              <span className="text-[10px] text-fg-subtle">@技能名 可手动召唤</span>
            </div>
          </div>
        </>
      )}
    </div>
  )
}
