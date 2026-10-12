// 新建本体宪章向导（宪章先行，ARCHITECTURE.md 决策 24）。
//
// 本体创建的唯一合法路径：用户在此确认设计宪章四字段 → 点「创建本体」
// → 空骨架本体 + 宪章原子落库（Rust 侧仅此 IPC 可创建本体；
// agent 的 import 工具对不存在的 IRI 一律拒绝）。
//
// AI 只起草不落笔：AI 起草建议预填表单，用户逐字段采纳/修改后亲自保存，
// 与「设计宪章」面板的用户主权红线一致（决策 22/23）。

import { useState } from "react";
import { Loader2, Sparkles, X } from "lucide-react";

import type { TtlCharter } from "@/lib/domain";
import { suggestCharter } from "@/lib/charter-suggest";
import { useProvider } from "@/hooks/useProvider";
import { useCreateOntologyWithCharter } from "@/hooks/useOntology";
import { useCreateOntologyTtlWithCharter } from "@/hooks/useOntologyTtl";

type CharterDraft = {
  business_scenario: string;
  business_essence: string;
};

const emptyCharter: CharterDraft = {
  business_scenario: "",
  business_essence: "",
};

const FIELD_META: {
  key: keyof CharterDraft;
  label: string;
  hint: string;
  placeholder: string;
}[] = [
  {
    key: "business_scenario",
    label: "业务场景",
    hint: "服务于什么业务、谁用、解决什么问题",
    placeholder: "例如：连接个人卖家、车商与买家的二手车交易平台……",
  },
  {
    key: "business_essence",
    label: "业务本质",
    hint: "核心对象/状态/关系/行为的一句话概括",
    placeholder: "例如：围绕非标实物的可信交易撮合……",
  },
];

export function CreateOntologyWizard({
  variant,
  open,
  onClose,
  onCreated,
}: {
  /** ttl = W3C Turtle（标识为 IRI）；palantir = Palantir 标准（标识为 PascalCase api_name） */
  variant: "ttl" | "palantir";
  open: boolean;
  onClose: () => void;
  /** 创建成功后回调（参数为本体 IRI / api_name），供父组件选中并刷新 */
  onCreated: (id: string) => void;
}) {
  const [id, setId] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [background, setBackground] = useState("");
  const [draft, setDraft] = useState<CharterDraft>(emptyCharter);
  const [questions, setQuestions] = useState<string[]>([]);
  const createPalantir = useCreateOntologyWithCharter();
  const createTtl = useCreateOntologyTtlWithCharter();
  const creating = createPalantir.isPending || createTtl.isPending;
  const [drafting, setDrafting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { data: provider } = useProvider();

  if (!open) return null;

  const idLabel = variant === "ttl" ? "本体 IRI" : "api_name（PascalCase）";
  const idValid =
    variant === "ttl"
      ? /^https?:\/\/\S+$/.test(id.trim())
      : /^[A-Z][a-zA-Z0-9]{0,99}$/.test(id.trim());
  const canCreate =
    idValid &&
    displayName.trim().length > 0 &&
    (draft.business_scenario.trim() || draft.business_essence.trim());

  function reset() {
    setId("");
    setDisplayName("");
    setBackground("");
    setDraft(emptyCharter);
    setQuestions([]);
    setError(null);
  }

  async function handleDraft() {
    setDrafting(true);
    setError(null);
    try {
      const p = provider ?? undefined;
      if (!p) throw new Error("未配置模型 Provider，无法 AI 起草");
      const s = await suggestCharter(p, {
        mode: "draft",
        draft: {
          ...draft,
          updated_by: "user",
          updated_at: 0,
        } as TtlCharter,
        entityContext: background,
      });
      setDraft({
        business_scenario: s.business_scenario ?? draft.business_scenario,
        business_essence: s.business_essence ?? draft.business_essence,
      });
      setQuestions(s.questions ?? []);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDrafting(false);
    }
  }

  function handleCreate() {
    setError(null);
    const charter = {
      ...draft,
      updated_by: "user",
      updated_at: 0,
    } as TtlCharter;
    const opts = {
      onSuccess: () => {
        onCreated(id.trim());
        onClose();
      },
      onError: (e: Error) => setError(e.message),
    };
    if (variant === "ttl") {
      createTtl.mutate(
        { iri: id.trim(), displayName: displayName.trim(), charter },
        opts,
      );
    } else {
      createPalantir.mutate(
        {
          apiName: id.trim(),
          displayName: displayName.trim(),
          description: background.trim(),
          charter,
        },
        opts,
      );
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div className="flex max-h-[90vh] w-full max-w-2xl flex-col rounded-lg border border-border bg-bg shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-2">
          <h2 className="flex items-center gap-1.5 text-sm font-semibold">
            <Sparkles size={14} className="text-accent" />
            新建本体——先完成设计宪章
          </h2>
          <button
            onClick={() => {
              reset();
              onClose();
            }}
            className="rounded-md p-1 text-fg-subtle hover:bg-bg-hover"
          >
            <X size={16} />
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          <p className="mb-3 text-[11px] text-fg-muted">
            设计宪章是本体建模的前提（不变点）：定义业务场景、本质、意图与约束，
            AI 建模与后续演进都以它为基线。创建后仍可在「设计宪章」面板调整。
          </p>

          <div className="grid grid-cols-2 gap-2">
            <label className="block">
              <span className="text-[10px] font-medium text-fg-subtle">
                {idLabel}
              </span>
              <input
                value={id}
                onChange={(e) => setId(e.target.value)}
                placeholder={
                  variant === "ttl"
                    ? "https://example.org/ontology/usedcar#UsedCarOntology"
                    : "UsedCarOntology"
                }
                className="mt-0.5 w-full rounded-md border border-border bg-bg-base px-2 py-1 text-xs outline-none"
              />
              {id.length > 0 && !idValid && (
                <span className="mt-0.5 block text-[10px] text-danger">
                  {variant === "ttl"
                    ? "需为合法 IRI（http/https 开头，无空格）"
                    : "需为 PascalCase（首字母大写，字母数字）"}
                </span>
              )}
            </label>
            <label className="block">
              <span className="text-[10px] font-medium text-fg-subtle">
                名称
              </span>
              <input
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder="例如：二手车电商本体"
                className="mt-0.5 w-full rounded-md border border-border bg-bg-base px-2 py-1 text-xs outline-none"
              />
            </label>
          </div>

          <label className="mt-2 block">
            <span className="text-[10px] font-medium text-fg-subtle">
              业务背景（可选——AI 起草的参考材料
              {variant === "palantir" ? "，同时作为本体描述落库" : ""}）
            </span>
            <textarea
              value={background}
              onChange={(e) => setBackground(e.target.value)}
              rows={2}
              placeholder="粘贴知识文档要点，或用一两句话描述业务……"
              className="mt-0.5 w-full resize-y rounded-md border border-border bg-bg-base px-2 py-1 text-xs outline-none"
            />
          </label>

          <div className="mt-3 flex items-center justify-between">
            <span className="text-xs font-medium text-fg">
              设计宪章（不变点）
            </span>
            <button
              onClick={handleDraft}
              disabled={drafting}
              className="flex items-center gap-1 rounded-md bg-accent/10 px-2 py-1 text-[11px] font-medium text-accent hover:bg-accent/20 disabled:opacity-50"
            >
              {drafting ? (
                <Loader2 size={11} className="animate-spin" />
              ) : (
                <Sparkles size={11} />
              )}
              AI 起草
            </button>
          </div>

          <div className="mt-1.5 space-y-2">
            {FIELD_META.map((f) => (
              <label key={f.key} className="block">
                <span className="flex items-baseline justify-between">
                  <span className="text-[10px] font-medium text-fg-subtle">
                    {f.label}
                  </span>
                  <span className="text-[10px] text-fg-subtle">{f.hint}</span>
                </span>
                <textarea
                  value={draft[f.key]}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      [f.key]: e.target.value,
                    })
                  }
                  rows={2}
                  placeholder={f.placeholder}
                  className="mt-0.5 w-full resize-y rounded-md border border-border bg-bg-base px-2 py-1 text-xs outline-none"
                />
              </label>
            ))}
          </div>

          {questions.length > 0 && (
            <div className="mt-2 rounded border border-border bg-bg-hover/30 p-2">
              <div className="text-[10px] font-medium text-fg-subtle">
                AI 需要你补充（不臆造业务事实）
              </div>
              <ul className="mt-0.5 list-disc pl-4 text-[11px] text-fg-muted">
                {questions.map((q, i) => (
                  <li key={i}>{q}</li>
                ))}
              </ul>
            </div>
          )}

          {error && (
            <p className="mt-2 whitespace-pre-wrap text-[11px] text-danger">
              {error}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-2">
          <button
            onClick={() => {
              reset();
              onClose();
            }}
            className="rounded-md px-3 py-1.5 text-xs text-fg-subtle hover:bg-bg-hover"
          >
            取消
          </button>
          <button
            onClick={handleCreate}
            disabled={!canCreate || creating}
            className="flex items-center gap-1 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:bg-accent/90 disabled:opacity-50"
          >
            {creating && <Loader2 size={12} className="animate-spin" />}
            创建本体
          </button>
        </div>
      </div>
    </div>
  );
}
