// UI 层：SlashMenu.tsx（线框 01 屏「入口：对话命令」斜杠命令菜单）
//
// 输入以 `/` 开头时弹出预置命令（参考 MentionMenu 的浮层形态）：
//   /validate  L0-L4 分层校验 + 多轮对抗（触发后切换到校验任务面板）
// 命令只是入口——执行过程在校验任务面板内展示，对话保留可回看。

const COMMANDS: { cmd: string; icon: string; desc: string }[] = [
  { cmd: "/validate", icon: "✅", desc: "L0-L4 分层校验 + 多轮对抗" },
];

export function SlashMenu({ onPick }: { onPick: (cmd: string) => void }) {
  return (
    <div className="absolute bottom-full left-2 z-10 mb-1 w-[320px] overflow-hidden rounded-lg border border-border bg-bg-elevated shadow-lg">
      {COMMANDS.map((c) => (
        <button
          key={c.cmd}
          onClick={() => onPick(c.cmd.slice(1))}
          className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs hover:bg-bg-hover"
        >
          <span>{c.icon}</span>
          <span className="font-mono font-semibold text-accent">{c.cmd}</span>
          <span className="truncate text-fg-subtle">{c.desc}</span>
        </button>
      ))}
      <div className="border-t border-border px-3 py-1.5 text-[11px] text-fg-subtle">
        回车执行第一条 · Esc 关闭菜单
      </div>
    </div>
  );
}
