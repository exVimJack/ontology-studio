// UI 层：FederationView.tsx（三期：联邦查询工作台）
// 数据源注册/管理 + schema 浏览 + 只读 SQL 编辑器 + 结果表格。
//
// 布局：左栏数据源列表 + 中栏 SQL 编辑器/结果 + 右栏 schema 浏览。
// 覆盖层模式（同 SettingsView），⌘Shift+F 或 Sidebar 底部入口打开。

import { useState } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  Database,
  Plus,
  Trash2,
  Play,
  Loader2,
  RefreshCw,
  Table2,
  ChevronRight,
  ChevronDown,
  X,
  CheckCircle2,
  XCircle,
  ArrowLeft,
} from "lucide-react";
import { useUiStore } from "@/stores/ui-store";
import { useIsMobile } from "@/hooks/useIsMobile";
import {
  useDataSources,
  useRegisterDataSource,
  useDeregisterDataSource,
  useTestDataSource,
  useFederationSchema,
  useExecuteQuery,
} from "@/hooks/useFederation";
import type {
  DataSourceConfig,
  DataSourceKind,
  DataSourceSummary,
} from "@/lib/domain";

// ── 数据源类型元数据 ──
const KIND_META: Record<DataSourceKind, { label: string; color: string }> = {
  mysql: { label: "MySQL", color: "text-sky-500" },
  postgres: { label: "PostgreSQL", color: "text-blue-500" },
  csv: { label: "CSV", color: "text-emerald-500" },
  excel: { label: "Excel", color: "text-green-500" },
};

export function FederationView() {
  const setFederationOpen = useUiStore((s) => s.setFederationOpen);
  const isMobile = useIsMobile();
  const { data: sources = [], isLoading, refetch } = useDataSources();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showRegister, setShowRegister] = useState(false);

  const selected = sources.find((s) => s.id === selectedId) ?? null;
  // 移动端分屏：无选中显示数据源列表，有选中显示 SQL 工作台（含返回按钮）
  const mobileShowWorkspace = isMobile && selected !== null;

  return (
    <div className="fixed inset-0 z-40 flex flex-col bg-bg max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)]">
      {/* 顶栏 */}
      <div className="flex items-center justify-between border-b border-border px-4 py-2">
        <div className="flex items-center gap-2">
          <Database size={16} className="text-accent" />
          <h1 className="text-sm font-semibold">联邦查询</h1>
          <span className="text-xs text-fg-subtle">三期 · DataFusion 54</span>
        </div>
        <div className="flex items-center gap-1">
          <button
            onClick={() => refetch()}
            className="rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover"
            title="刷新"
          >
            <RefreshCw size={14} />
          </button>
          <button
            onClick={() => setFederationOpen(false)}
            className="rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        {/* 左栏：数据源列表（移动端：无选中时全宽显示） */}
        <div
          className={`flex flex-col border-r border-border ${isMobile ? (mobileShowWorkspace ? "hidden" : "w-full") : "w-64"}`}
        >
          {isMobile && mobileShowWorkspace ? null : (
            <>
              <div className="flex items-center justify-between px-3 py-2">
                <span className="text-xs font-medium uppercase tracking-wide text-fg-subtle">
                  数据源
                </span>
                <button
                  onClick={() => setShowRegister(true)}
                  className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-accent hover:bg-bg-hover"
                >
                  <Plus size={12} /> 新建
                </button>
              </div>
              <div className="flex-1 overflow-y-auto px-1">
                {isLoading ? (
                  <div className="flex justify-center py-8">
                    <Loader2
                      className="animate-spin text-fg-subtle"
                      size={16}
                    />
                  </div>
                ) : sources.length === 0 ? (
                  <div className="px-3 py-8 text-center text-xs text-fg-subtle">
                    暂无数据源
                    <br />
                    点击「新建」注册
                  </div>
                ) : (
                  sources.map((s) => (
                    <SourceItem
                      key={s.id}
                      src={s}
                      active={s.id === selectedId}
                      onClick={() => setSelectedId(s.id)}
                    />
                  ))
                )}
              </div>
            </>
          )}
        </div>

        {/* 中栏：SQL 编辑器 + 结果（移动端：有选中时全宽显示 + 返回按钮） */}
        <div
          className={`flex min-w-0 flex-1 flex-col ${isMobile && !mobileShowWorkspace ? "hidden" : "flex"}`}
        >
          {isMobile && mobileShowWorkspace && (
            <button
              onClick={() => setSelectedId(null)}
              className="flex items-center gap-1.5 border-b border-border px-3 py-2 text-xs text-fg-muted hover:bg-bg-hover"
            >
              <ArrowLeft size={14} />
              返回数据源
            </button>
          )}
          {selected ? (
            <SqlWorkspace catalog={selected.name} />
          ) : !isMobile ? (
            <div className="flex h-full items-center justify-center text-sm text-fg-subtle">
              选择左侧数据源开始查询
            </div>
          ) : null}
        </div>
      </div>

      {/* 注册表单弹层 */}
      {showRegister && (
        <RegisterDialog onClose={() => setShowRegister(false)} />
      )}
    </div>
  );
}

// ── 数据源列表项 ──
function SourceItem({
  src,
  active,
  onClick,
}: {
  src: DataSourceSummary;
  active: boolean;
  onClick: () => void;
}) {
  const meta = KIND_META[src.kind];
  const dereg = useDeregisterDataSource();
  return (
    <div
      onClick={onClick}
      className={`group mx-1 flex cursor-pointer items-center gap-2 rounded-md px-2 py-2 ${
        active ? "bg-bg-hover" : "hover:bg-bg-hover/50"
      }`}
    >
      <Database size={14} className={`shrink-0 ${meta.color}`} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm">{src.name}</div>
        <div className="flex items-center gap-1.5 text-[10px] text-fg-subtle">
          {src.connected ? (
            <>
              <CheckCircle2 size={10} className="text-emerald-500" />
              {src.table_count ?? 0} 表
            </>
          ) : (
            <>
              <XCircle size={10} className="text-rose-500" />
              未连接
            </>
          )}
        </div>
      </div>
      <button
        onClick={async (e) => {
          e.stopPropagation();
          const ok = await confirm(`注销数据源「${src.name}」？`, {
            kind: "warning",
          });
          if (ok) dereg.mutate(src.id);
        }}
        className="shrink-0 rounded p-1 text-fg-subtle opacity-0 hover:bg-bg hover:text-danger group-hover:opacity-100"
        title="注销"
      >
        <Trash2 size={12} />
      </button>
    </div>
  );
}

// ── SQL 工作区 ──
function SqlWorkspace({ catalog }: { catalog: string }) {
  const [sql, setSql] = useState(`SELECT * FROM ${catalog}.public.`);
  const [limit, setLimit] = useState(200);
  const exec = useExecuteQuery();
  const { data: schema, isLoading: schemaLoading } =
    useFederationSchema(catalog);

  const run = () => {
    if (!sql.trim()) return;
    exec.mutate({ sql, limit });
  };

  return (
    <div className="flex min-h-0 flex-1">
      {/* SQL 编辑器 + 结果 */}
      <div className="flex min-w-0 flex-1 flex-col">
        {/* 编辑器 */}
        <div className="border-b border-border p-2">
          <textarea
            value={sql}
            onChange={(e) => setSql(e.target.value)}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                e.preventDefault();
                run();
              }
            }}
            placeholder="输入只读 SQL（⌘/Ctrl+Enter 执行）"
            className="h-24 w-full resize-none rounded-md border border-border bg-bg p-2 font-mono text-xs outline-none focus:border-accent"
            spellCheck={false}
          />
          <div className="mt-1 flex items-center justify-between">
            <div className="flex items-center gap-2 text-xs text-fg-subtle">
              <label className="flex items-center gap-1">
                LIMIT
                <input
                  type="number"
                  value={limit}
                  onChange={(e) => setLimit(Number(e.target.value) || 200)}
                  className="w-16 rounded border border-border bg-bg px-1 py-0.5 text-xs outline-none focus:border-accent"
                />
              </label>
            </div>
            <button
              onClick={run}
              disabled={exec.isPending}
              className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-accent-fg hover:opacity-90 disabled:opacity-50"
            >
              {exec.isPending ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Play size={12} />
              )}
              执行
            </button>
          </div>
          {exec.error && (
            <div className="mt-1 rounded-md bg-rose-500/10 px-2 py-1 text-xs text-rose-500">
              {exec.error instanceof Error
                ? exec.error.message
                : String(exec.error)}
            </div>
          )}
        </div>
        {/* 结果 */}
        <div className="min-h-0 flex-1 overflow-auto">
          {exec.data ? (
            <ResultTable
              rows={exec.data.rows}
              columns={exec.data.columns}
              elapsed={exec.data.elapsed_ms}
              rowCount={exec.data.row_count}
            />
          ) : (
            <div className="flex h-full items-center justify-center text-xs text-fg-subtle">
              执行查询后结果显示在此
            </div>
          )}
        </div>
      </div>

      {/* 右栏：schema 浏览 */}
      <div className="w-72 overflow-y-auto border-l border-border p-2">
        <div className="mb-2 text-xs font-medium uppercase tracking-wide text-fg-subtle">
          表结构
        </div>
        {schemaLoading ? (
          <Loader2 className="animate-spin text-fg-subtle" size={14} />
        ) : schema?.tables ? (
          <SchemaTree
            tables={schema.tables}
            onInsert={(t) =>
              setSql(`SELECT * FROM ${catalog}.public.${t} LIMIT 100`)
            }
          />
        ) : (
          <div className="text-xs text-fg-subtle">无表</div>
        )}
      </div>
    </div>
  );
}

// ── 结果表格 ──
function ResultTable({
  rows,
  columns,
  elapsed,
  rowCount,
}: {
  rows: string[];
  columns: { name: string; data_type: string; nullable: boolean }[];
  elapsed: number;
  rowCount: number;
}) {
  const parsed = rows.map((r) => {
    try {
      return JSON.parse(r) as Record<string, unknown>;
    } catch {
      return {} as Record<string, unknown>;
    }
  });
  return (
    <div className="flex h-full flex-col">
      <div className="border-b border-border px-3 py-1 text-xs text-fg-subtle">
        {rowCount} 行 · {elapsed}ms
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full text-xs">
          <thead className="sticky top-0 bg-bg-elevated">
            <tr>
              {columns.map((c) => (
                <th
                  key={c.name}
                  className="border-b border-border px-2 py-1 text-left font-medium text-fg-muted"
                >
                  {c.name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {parsed.map((row, i) => (
              <tr key={i} className="hover:bg-bg-hover/50">
                {columns.map((c) => (
                  <td
                    key={c.name}
                    className="border-b border-border/50 px-2 py-1 font-mono"
                  >
                    {formatCell(row[c.name])}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function formatCell(v: unknown): string {
  if (v == null) return "NULL";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

// ── Schema 树 ──
function SchemaTree({
  tables,
  onInsert,
}: {
  tables: {
    name: string;
    schema?: string;
    columns?: { name: string; data_type: string; nullable: boolean }[];
  }[];
  onInsert: (table: string) => void;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const toggle = (n: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (next.has(n)) next.delete(n);
      else next.add(n);
      return next;
    });
  return (
    <div className="space-y-0.5">
      {tables.map((t) => {
        const key = t.name;
        const isOpen = expanded.has(key);
        return (
          <div key={key}>
            <div className="flex items-center group">
              <button
                onClick={() => toggle(key)}
                className="flex items-center gap-1 rounded px-1 py-0.5 text-xs hover:bg-bg-hover"
              >
                {isOpen ? (
                  <ChevronDown size={12} />
                ) : (
                  <ChevronRight size={12} />
                )}
                <Table2 size={12} className="text-fg-subtle" />
                <span>{t.name}</span>
              </button>
              <button
                onClick={() => onInsert(t.name)}
                className="ml-auto rounded p-0.5 text-fg-subtle opacity-0 hover:bg-bg group-hover:opacity-100"
                title="插入 SELECT *"
              >
                <Play size={10} />
              </button>
            </div>
            {isOpen && t.columns && (
              <div className="ml-6 border-l border-border pl-2">
                {t.columns.map((c) => (
                  <div
                    key={c.name}
                    className="flex items-center gap-1 py-0.5 text-[11px]"
                  >
                    <span className="text-fg-muted">{c.name}</span>
                    <span className="text-fg-subtle">{c.data_type}</span>
                    {c.nullable && <span className="text-fg-subtle/60">?</span>}
                  </div>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

// ── 注册对话框 ──
function RegisterDialog({ onClose }: { onClose: () => void }) {
  const register = useRegisterDataSource();
  const test = useTestDataSource();
  const [kind, setKind] = useState<DataSourceKind>("mysql");
  const [name, setName] = useState("");
  // DB 字段
  const [host, setHost] = useState("localhost");
  const [port, setPort] = useState(3306);
  const [database, setDatabase] = useState("");
  const [username, setUsername] = useState("root");
  const [password, setPassword] = useState("");
  const [sslMode, setSslMode] = useState("require");
  // 文件字段
  const [path, setPath] = useState("");
  const [hasHeader, setHasHeader] = useState(true);
  const [delimiter, setDelimiter] = useState(",");
  const [color, setColor] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<string | null>(null);

  const isDb = kind === "mysql" || kind === "postgres";

  const buildConfig = (): DataSourceConfig => {
    const id = crypto.randomUUID();
    const conn = isDb
      ? {
          kind,
          params: {
            host,
            port,
            database,
            username,
            password: password || null,
            ssl_mode: sslMode,
          },
        }
      : {
          kind,
          params: { path, has_header: hasHeader, delimiter: delimiter || "," },
        };
    return {
      id,
      name,
      connection: conn as DataSourceConfig["connection"],
      color,
      created_at: Date.now(),
    };
  };

  const onTest = async () => {
    if (!name.trim()) return setTestResult("错误：请填写名称");
    setTestResult(null);
    try {
      const snap = await test.mutateAsync(buildConfig());
      setTestResult(`✓ 连接成功，发现 ${snap.tables.length} 个表`);
    } catch (e) {
      setTestResult(`✗ ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const onRegister = async () => {
    if (!name.trim()) return setTestResult("错误：请填写名称");
    try {
      await register.mutateAsync(buildConfig());
      onClose();
    } catch (e) {
      setTestResult(`✗ ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onClose}
    >
      <div
        className="max-h-[90vh] w-[480px] overflow-y-auto rounded-lg border border-border bg-bg p-4 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">注册数据源</h2>
          <button
            onClick={onClose}
            className="rounded p-1 text-fg-subtle hover:bg-bg-hover"
          >
            <X size={14} />
          </button>
        </div>

        <div className="space-y-3">
          {/* 类型选择 */}
          <div>
            <Label>类型</Label>
            <div className="flex gap-1">
              {(Object.keys(KIND_META) as DataSourceKind[]).map((k) => (
                <button
                  key={k}
                  onClick={() => {
                    setKind(k);
                    if (k === "postgres") setPort(5432);
                    if (k === "mysql") setPort(3306);
                  }}
                  className={`flex-1 rounded-md border px-2 py-1 text-xs ${
                    kind === k
                      ? "border-accent bg-accent/10 text-accent"
                      : "border-border text-fg-muted"
                  }`}
                >
                  {KIND_META[k].label}
                </button>
              ))}
            </div>
          </div>

          {/* 名称 */}
          <div>
            <Label>
              名称 <span className="text-rose-500">*</span>
            </Label>
            <Input
              value={name}
              onChange={setName}
              placeholder="如 prod_db（同时作为 catalog 名）"
            />
          </div>

          {/* 连接参数 */}
          {isDb ? (
            <>
              <div className="grid grid-cols-3 gap-2">
                <div className="col-span-2">
                  <Label>主机</Label>
                  <Input value={host} onChange={setHost} />
                </div>
                <div>
                  <Label>端口</Label>
                  <Input
                    value={String(port)}
                    onChange={(v) => setPort(Number(v) || 0)}
                  />
                </div>
              </div>
              <div>
                <Label>数据库</Label>
                <Input value={database} onChange={setDatabase} />
              </div>
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <Label>用户名</Label>
                  <Input value={username} onChange={setUsername} />
                </div>
                <div>
                  <Label>密码</Label>
                  <Input
                    value={password}
                    onChange={setPassword}
                    type="password"
                  />
                </div>
              </div>
              <div>
                <Label>SSL</Label>
                <select
                  value={sslMode}
                  onChange={(e) => setSslMode(e.target.value)}
                  className="w-full rounded-md border border-border bg-bg px-2 py-1 text-xs outline-none focus:border-accent"
                >
                  <option value="disable">disable</option>
                  <option value="require">require</option>
                  <option value="verify">verify</option>
                </select>
              </div>
            </>
          ) : (
            <>
              <div>
                <Label>
                  文件路径 <span className="text-rose-500">*</span>
                </Label>
                <Input
                  value={path}
                  onChange={setPath}
                  placeholder="CSV 可填目录（注册全部 .csv）"
                />
              </div>
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <Label>分隔符</Label>
                  <Input
                    value={delimiter}
                    onChange={setDelimiter}
                    placeholder=","
                  />
                </div>
                <div className="flex items-end pb-1">
                  <label className="flex items-center gap-1 text-xs">
                    <input
                      type="checkbox"
                      checked={hasHeader}
                      onChange={(e) => setHasHeader(e.target.checked)}
                    />
                    有表头
                  </label>
                </div>
              </div>
            </>
          )}

          {/* 颜色标记 */}
          <div>
            <Label>颜色标记</Label>
            <div className="flex gap-1">
              {[
                { v: null, label: "无" },
                { v: "rose", label: "生产" },
                { v: "sky", label: "测试" },
                { v: "emerald", label: "本地" },
              ].map((c) => (
                <button
                  key={c.label}
                  onClick={() => setColor(c.v)}
                  className={`rounded-md border px-2 py-1 text-xs ${
                    color === c.v ? "border-accent" : "border-border"
                  }`}
                >
                  {c.label}
                </button>
              ))}
            </div>
          </div>

          {/* 测试结果 */}
          {testResult && (
            <div
              className={`rounded-md px-2 py-1 text-xs ${
                testResult.startsWith("✓")
                  ? "bg-emerald-500/10 text-emerald-500"
                  : "bg-rose-500/10 text-rose-500"
              }`}
            >
              {testResult}
            </div>
          )}

          {/* 操作 */}
          <div className="flex justify-end gap-2 pt-2">
            <button
              onClick={onTest}
              disabled={test.isPending}
              className="rounded-md border border-border px-3 py-1 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              {test.isPending ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                "测试连接"
              )}
            </button>
            <button
              onClick={onRegister}
              disabled={register.isPending}
              className="rounded-md bg-accent px-3 py-1 text-xs text-accent-fg hover:opacity-90 disabled:opacity-50"
            >
              {register.isPending ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                "注册"
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── 小组件 ──
function Label({ children }: { children: React.ReactNode }) {
  return (
    <label className="mb-1 block text-[11px] font-medium text-fg-muted">
      {children}
    </label>
  );
}
function Input({
  value,
  onChange,
  placeholder,
  type = "text",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
}) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className="w-full rounded-md border border-border bg-bg px-2 py-1 text-xs outline-none focus:border-accent"
    />
  );
}
