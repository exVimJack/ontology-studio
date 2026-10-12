/**
 * 平台相关快捷键显示（§：UI 文案不硬编码 ⌘）。
 * 快捷键处理层统一 e.metaKey || e.ctrlKey，此处只负责「显示」：
 *   macOS   → ⌘K / ⌘⇧M / ⌘↵（Apple 风格符号，无 + 号）
 *   Windows/Linux → Ctrl+K / Ctrl+Shift+M / Ctrl+Enter（+ 号连接）
 */

export type ShortcutKey = "mod" | "shift" | string;

function isMacPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);
}

/**
 * 组合快捷键显示，如 `shortcut(["mod", "K"])` → "⌘K" (mac) / "Ctrl+K" (win)。
 *
 * 特殊键映射：
 *   - "Enter" → mac 显示 "↵"，其它显示 "Enter"
 *   - "mod"   → mac 显示 "⌘"，其它显示 "Ctrl"
 *   - "shift" → mac 显示 "⇧"，其它显示 "Shift"
 */
export function shortcut(keys: ShortcutKey[]): string {
  const mac = isMacPlatform();
  const mapped = keys.map((k) => {
    if (k === "mod") return mac ? "⌘" : "Ctrl";
    if (k === "shift") return mac ? "⇧" : "Shift";
    if (k === "Enter") return mac ? "↵" : "Enter";
    return k;
  });
  return mac ? mapped.join("") : mapped.join("+");
}